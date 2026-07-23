using SharpGen.Runtime;
using System.Diagnostics;
using System.Runtime.InteropServices;
using Vortice;
using Vortice.Direct3D11;
using Vortice.DXGI;
using Vortice.MediaFoundation;

namespace ReceiverNative;

internal sealed class MediaFoundationD3D11PlaybackController : IPlaybackController
{
    private readonly D3D11SwapChainRenderer _renderer;
    private readonly Dictionary<long, RoiMetadata> _enhancementMetadataByPts = new();

    private LatestAccessUnitQueue? _queue;
    private LatestAccessUnitQueue? _enhancementQueue;
    private SessionConfig? _sessionConfig;
    private HardwareDecodeMode _hardwareDecodeMode = HardwareDecodeMode.Auto;
    private bool _aggressiveMode = true;
    private bool _ultraLowLatencyMode = true;
    private CancellationTokenSource? _decodeCts;
    private Task? _decodeTask;
    private CancellationTokenSource? _enhancementDecodeCts;
    private Task? _enhancementDecodeTask;
    private MediaFoundationDecoderSession? _decoderSession;
    private MediaFoundationEnhancementDecoderSession? _enhancementDecoderSession;
    private bool _disposed;
    private string _currentStatus = "Idle";
    private long _sampleTimeHns;
    private long _sampleDurationHns = 166_666;
    private long? _basePresentationTimeUs;
    private long _lastNormalizedPresentationTimeHns;
    private long? _pacingBaseSampleTimeHns;
    private long _pacingBaseTicks;
    private bool _softwareSafeMode;
    private long _lastSilentResetAtTicks;
    private int _silentResetBurstCount;
    private TimeSpan _adaptiveJitterDelay = TimeSpan.Zero;
    private long _lastBasePresentationTimeUs;
    private long _enhancementSampleTimeHns;
    private long? _enhancementPresentationBaseUs;
    private long _lastEnhancementNormalizedTimeHns;

    public MediaFoundationD3D11PlaybackController(Control playbackHost)
    {
        MediaFoundationRuntime.Acquire();
        _renderer = new D3D11SwapChainRenderer(playbackHost);
        _renderer.FramePresented += ticks => FramePresented?.Invoke(ticks);
    }

    public event Action<string>? StatusChanged;
    public event Action<PlaybackStreamStats>? StreamStatsChanged;
    public event Action<PlaybackStreamStats>? EnhancementStreamStatsChanged;
    public event Action<long>? FrameDecoded;
    public event Action<long>? FramePresented;

    public string BackendLabel => PlaybackBackendKind.MediaFoundationDirect3D11.ToUiLabel();

    public void UpdateHardwareDecodeMode(HardwareDecodeMode mode)
    {
        if (_hardwareDecodeMode == mode)
        {
            return;
        }

        _hardwareDecodeMode = mode;
        RestartIfConfigured();
    }

    public void UpdateAggressiveMode(bool enabled)
    {
        if (_aggressiveMode == enabled)
        {
            return;
        }

        _aggressiveMode = enabled;
        RestartIfConfigured();
    }

    public void UpdateUltraLowLatencyMode(bool enabled)
    {
        if (_ultraLowLatencyMode == enabled)
        {
            return;
        }

        _ultraLowLatencyMode = enabled;
        RestartIfConfigured();
    }

    public void UpdateAdaptiveJitterBuffer(TimeSpan delay)
    {
        _adaptiveJitterDelay = delay > TimeSpan.Zero ? delay : TimeSpan.Zero;
        _queue?.UpdateJitterBufferDelay(_adaptiveJitterDelay);
    }

    public void ApplySessionConfig(SessionConfig config)
    {
        var mustRestart = _sessionConfig is null ||
            !string.Equals(_sessionConfig.Codec, config.Codec, StringComparison.OrdinalIgnoreCase) ||
            _sessionConfig.Width != config.Width ||
            _sessionConfig.Height != config.Height ||
            _sessionConfig.IsSplitStream != config.IsSplitStream ||
            _sessionConfig.EnhancementMaxWidth != config.EnhancementMaxWidth ||
            _sessionConfig.EnhancementMaxHeight != config.EnhancementMaxHeight ||
            !string.Equals(_sessionConfig.EnhancementCodec, config.EnhancementCodec, StringComparison.OrdinalIgnoreCase);

        _sessionConfig = config;
        _sampleDurationHns = config.Fps > 0
            ? Math.Max(1, 10_000_000L / config.Fps)
            : 166_666L;
        _sampleTimeHns = 0;
        _basePresentationTimeUs = null;
        _lastNormalizedPresentationTimeHns = 0;
        _pacingBaseSampleTimeHns = null;
        _pacingBaseTicks = 0;
        _softwareSafeMode = false;
        _lastSilentResetAtTicks = 0;
        _silentResetBurstCount = 0;
        _lastBasePresentationTimeUs = 0;
        _enhancementSampleTimeHns = 0;
        _enhancementPresentationBaseUs = null;
        _lastEnhancementNormalizedTimeHns = 0;
        lock (_enhancementMetadataByPts)
        {
            _enhancementMetadataByPts.Clear();
        }

        if (mustRestart)
        {
            RestartPlayback();
        }
    }

    public void EnqueueAccessUnit(byte[] bytes, bool isKeyFrame, long presentationTimeUs)
    {
        _queue?.Enqueue(bytes, isKeyFrame, presentationTimeUs);
    }

    public void EnqueueEnhancementAccessUnit(byte[] bytes, bool isKeyFrame, long presentationTimeUs, RoiMetadata? metadata)
    {
        if (!(_sessionConfig?.IsSplitStream ?? false) || _enhancementQueue is null || metadata is null)
        {
            return;
        }

        lock (_enhancementMetadataByPts)
        {
            _enhancementMetadataByPts[presentationTimeUs] = metadata;
            while (_enhancementMetadataByPts.Count > 24)
            {
                var firstKey = _enhancementMetadataByPts.Keys.OrderBy(static key => key).First();
                _enhancementMetadataByPts.Remove(firstKey);
            }
        }
        _enhancementQueue.Enqueue(bytes, isKeyFrame, presentationTimeUs);
    }

    public void WaitForKeyFrame()
    {
        _queue?.WaitForKeyFrame();
        _decoderSession?.Flush();
        ResetEnhancementPath();
    }

    public void ResetEnhancementPath()
    {
        _enhancementQueue?.WaitForKeyFrame();
        _enhancementDecoderSession?.Flush();
        lock (_enhancementMetadataByPts)
        {
            _enhancementMetadataByPts.Clear();
        }
        _enhancementSampleTimeHns = 0;
        _enhancementPresentationBaseUs = null;
        _lastEnhancementNormalizedTimeHns = 0;
    }

    public void Dispose()
    {
        if (_disposed)
        {
            return;
        }

        _disposed = true;
        StopWorker();
        _queue?.Dispose();
        _renderer.Dispose();
        MediaFoundationRuntime.Release();
    }

    private void RestartIfConfigured()
    {
        if (_sessionConfig is null)
        {
            return;
        }

        RestartPlayback();
    }

    private void RestartPlayback()
    {
        if (_sessionConfig is null || _disposed)
        {
            return;
        }

        StopWorker();
        _queue?.Dispose();
        _enhancementQueue?.Dispose();
        lock (_enhancementMetadataByPts)
        {
            _enhancementMetadataByPts.Clear();
        }

        var isAdbTunnel = string.Equals(_sessionConfig.Transport, "EVRT_REALTIME_V2_TCP_ADB", StringComparison.OrdinalIgnoreCase);
        var lowLatencyDecodeProfile =
            _ultraLowLatencyMode ||
            _aggressiveMode ||
            _hardwareDecodeMode.IsNvidiaLowLatencyProfile() ||
            _hardwareDecodeMode.IsIntelQuickSync();
        var allowUdpHeadroom = !isAdbTunnel && lowLatencyDecodeProfile;
        var jitterBufferDelay = !isAdbTunnel && _ultraLowLatencyMode
            ? _adaptiveJitterDelay
            : TimeSpan.Zero;

        _queue = new LatestAccessUnitQueue(
            maxQueuedUnits: lowLatencyDecodeProfile ? (allowUdpHeadroom ? 2 : 1) : 2,
            maxQueuedBytes: isAdbTunnel
                ? (_ultraLowLatencyMode ? 384 * 1024 : (lowLatencyDecodeProfile ? 512 * 1024 : 768 * 1024))
                : (_ultraLowLatencyMode ? (allowUdpHeadroom ? 640 * 1024 : 512 * 1024) : (lowLatencyDecodeProfile ? 768 * 1024 : 1024 * 1024)),
            statsChanged: stats => StreamStatsChanged?.Invoke(stats),
            hardResetOnKeyFrame: isAdbTunnel,
            dropCurrentOnWaitForKeyFrame: true,
            preferLatestQueuedUnit: !isAdbTunnel && _ultraLowLatencyMode,
            jitterBufferDelay: jitterBufferDelay);

        _decodeCts = new CancellationTokenSource();
        _decoderSession = CreateDecoderSession(forceSoftwareSafePath: _softwareSafeMode);
        _decodeTask = Task.Run(() => DecodeLoopAsync(_decodeCts.Token));
        if (_sessionConfig.IsSplitStream)
        {
            _enhancementQueue = new LatestAccessUnitQueue(
                maxQueuedUnits: 1,
                maxQueuedBytes: 384 * 1024,
                statsChanged: stats => EnhancementStreamStatsChanged?.Invoke(stats),
                hardResetOnKeyFrame: true,
                dropCurrentOnWaitForKeyFrame: true,
                preferLatestQueuedUnit: true,
                jitterBufferDelay: TimeSpan.Zero);
            _enhancementDecodeCts = new CancellationTokenSource();
            _enhancementDecoderSession = CreateEnhancementDecoderSession(forceSoftwareSafePath: false);
            _enhancementDecodeTask = Task.Run(() => DecodeEnhancementLoopAsync(_enhancementDecodeCts.Token));
        }
        UpdateStatus(_softwareSafeMode ? "Opening (MF software-safe)" : "Opening");
        _renderer.Clear();
    }

    private async Task DecodeLoopAsync(CancellationToken token)
    {
        try
        {
            while (!token.IsCancellationRequested)
            {
                if (_queue is null || _decoderSession is null)
                {
                    return;
                }

                if (!_queue.TryDequeue(token, out var bytes, out var isKeyFrame, out var presentationTimeUs) || bytes is null)
                {
                    return;
                }

                try
                {
                    var sampleTimeHns = NormalizePresentationTime(presentationTimeUs);
                    await PaceFrameIfEarlyAsync(sampleTimeHns, token);
                    _decoderSession.ProcessAccessUnit(bytes, isKeyFrame, sampleTimeHns, _sampleDurationHns);
                    _sampleTimeHns = sampleTimeHns + _sampleDurationHns;
                    _lastBasePresentationTimeUs = presentationTimeUs;
                    FrameDecoded?.Invoke(Stopwatch.GetTimestamp());
                    UpdateStatus("Playing");
                }
                catch (OperationCanceledException)
                {
                    return;
                }
                catch (Exception ex)
                {
                    if (TrySilentReset(ex))
                    {
                        continue;
                    }
                    if (TrySwitchToSoftwareSafeMode(ex))
                    {
                        continue;
                    }
                    UpdateStatus($"MF decode error: {ex.Message}");
                    _queue.WaitForKeyFrame();
                    _decoderSession.Flush();
                    await Task.Delay(5, token);
                }
            }
        }
        catch (OperationCanceledException)
        {
        }
    }

    private async Task DecodeEnhancementLoopAsync(CancellationToken token)
    {
        try
        {
            while (!token.IsCancellationRequested)
            {
                if (_enhancementQueue is null || _enhancementDecoderSession is null)
                {
                    return;
                }

                if (!_enhancementQueue.TryDequeue(token, out var bytes, out var isKeyFrame, out var presentationTimeUs) || bytes is null)
                {
                    return;
                }

                try
                {
                    var sampleTimeHns = NormalizeEnhancementPresentationTime(presentationTimeUs);
                    _enhancementDecoderSession.ProcessAccessUnit(bytes, isKeyFrame, sampleTimeHns, _sampleDurationHns);
                    _enhancementSampleTimeHns = sampleTimeHns + _sampleDurationHns;
                }
                catch (OperationCanceledException)
                {
                    return;
                }
                catch
                {
                    ResetEnhancementPath();
                    await Task.Delay(5, token);
                }
            }
        }
        catch (OperationCanceledException)
        {
        }
    }

    private void StopWorker()
    {
        var cts = _decodeCts;
        var task = _decodeTask;
        var enhancementCts = _enhancementDecodeCts;
        var enhancementTask = _enhancementDecodeTask;

        _decodeCts = null;
        _decodeTask = null;
        _enhancementDecodeCts = null;
        _enhancementDecodeTask = null;

        if (cts is not null)
        {
            cts.Cancel();
        }
        if (enhancementCts is not null)
        {
            enhancementCts.Cancel();
        }

        try
        {
            task?.Wait(250);
            enhancementTask?.Wait(250);
        }
        catch
        {
        }

        _decoderSession?.Dispose();
        _decoderSession = null;
        _enhancementDecoderSession?.Dispose();
        _enhancementDecoderSession = null;
        lock (_enhancementMetadataByPts)
        {
            _enhancementMetadataByPts.Clear();
        }
        cts?.Dispose();
        enhancementCts?.Dispose();
    }

    private bool TrySilentReset(Exception originalError)
    {
        if (_disposed || _sessionConfig is null)
        {
            return false;
        }

        var nowTicks = Stopwatch.GetTimestamp();
        var resetWindow = TimeSpan.FromSeconds(2);
        if (ElapsedSince(_lastSilentResetAtTicks, nowTicks) <= resetWindow)
        {
            _silentResetBurstCount += 1;
        }
        else
        {
            _silentResetBurstCount = 1;
        }
        _lastSilentResetAtTicks = nowTicks;

        if (_silentResetBurstCount > 2)
        {
            return false;
        }

        try
        {
            _queue?.WaitForKeyFrame();
            _decoderSession?.Dispose();
            _decoderSession = CreateDecoderSession(forceSoftwareSafePath: _softwareSafeMode);
        _sampleTimeHns = 0;
        _basePresentationTimeUs = null;
        _lastNormalizedPresentationTimeHns = 0;
        _pacingBaseSampleTimeHns = null;
        _pacingBaseTicks = 0;
        _lastBasePresentationTimeUs = 0;
        _enhancementSampleTimeHns = 0;
        _enhancementPresentationBaseUs = null;
        _lastEnhancementNormalizedTimeHns = 0;
            UpdateStatus("MF silent reset");
            return true;
        }
        catch (Exception resetError)
        {
            UpdateStatus($"MF decode error: {originalError.Message}; silent reset failed: {resetError.Message}");
            return false;
        }
    }

    private MediaFoundationDecoderSession CreateDecoderSession(bool forceSoftwareSafePath)
    {
        if (_sessionConfig is null)
        {
            throw new InvalidOperationException("Session config is not available for Media Foundation decoder");
        }

        try
        {
            return new MediaFoundationDecoderSession(
                _renderer,
                _sessionConfig,
                _hardwareDecodeMode,
                _ultraLowLatencyMode,
                forceSoftwareSafePath);
        }
        catch
        {
            if (forceSoftwareSafePath)
            {
                throw;
            }

            _softwareSafeMode = true;
            return new MediaFoundationDecoderSession(
                _renderer,
                _sessionConfig,
                _hardwareDecodeMode,
                _ultraLowLatencyMode,
                forceSoftwareSafePath: true);
        }
    }

    private MediaFoundationEnhancementDecoderSession CreateEnhancementDecoderSession(bool forceSoftwareSafePath)
    {
        if (_sessionConfig is null)
        {
            throw new InvalidOperationException("Session config is not available for Media Foundation enhancement decoder");
        }

        return new MediaFoundationEnhancementDecoderSession(
            _sessionConfig,
            _hardwareDecodeMode,
            forceSoftwareSafePath,
            HandleEnhancementFrameDecoded);
    }

    private bool TrySwitchToSoftwareSafeMode(Exception originalError)
    {
        if (_softwareSafeMode || _sessionConfig is null || _disposed)
        {
            return false;
        }

        var errorMessage = originalError.Message;
        if (errorMessage.Contains("0x80070057", StringComparison.OrdinalIgnoreCase) ||
            errorMessage.Contains("E_INVALIDARG", StringComparison.OrdinalIgnoreCase))
        {
            return false;
        }

        try
        {
            _softwareSafeMode = true;
            _decoderSession?.Dispose();
            _decoderSession = CreateDecoderSession(forceSoftwareSafePath: true);
            _queue?.WaitForKeyFrame();
            _sampleTimeHns = 0;
            _pacingBaseSampleTimeHns = null;
            _pacingBaseTicks = 0;
            UpdateStatus("MF software-safe recovery");
            return true;
        }
        catch (Exception fallbackError)
        {
            UpdateStatus($"MF decode error: {originalError.Message}; software-safe failed: {fallbackError.Message}");
            return false;
        }
    }

    private void UpdateStatus(string status)
    {
        if (string.Equals(_currentStatus, status, StringComparison.Ordinal))
        {
            return;
        }

        _currentStatus = status;
        StatusChanged?.Invoke(status);
    }

    private long NormalizePresentationTime(long presentationTimeUs)
    {
        if (presentationTimeUs <= 0)
        {
            return _sampleTimeHns;
        }

        _basePresentationTimeUs ??= presentationTimeUs;
        var normalizedHns = Math.Max(0, (presentationTimeUs - _basePresentationTimeUs.Value) * 10L);
        if (normalizedHns <= _lastNormalizedPresentationTimeHns)
        {
            normalizedHns = _lastNormalizedPresentationTimeHns + _sampleDurationHns;
        }

        _lastNormalizedPresentationTimeHns = normalizedHns;
        return normalizedHns;
    }

    private long NormalizeEnhancementPresentationTime(long presentationTimeUs)
    {
        if (presentationTimeUs <= 0)
        {
            return _enhancementSampleTimeHns;
        }

        _enhancementPresentationBaseUs ??= _basePresentationTimeUs ?? presentationTimeUs;
        var normalizedHns = Math.Max(0, (presentationTimeUs - _enhancementPresentationBaseUs.Value) * 10L);
        if (normalizedHns <= _lastEnhancementNormalizedTimeHns)
        {
            normalizedHns = _lastEnhancementNormalizedTimeHns + _sampleDurationHns;
        }

        _lastEnhancementNormalizedTimeHns = normalizedHns;
        return normalizedHns;
    }

    private void HandleEnhancementFrameDecoded(DecodedEnhancementFrame frame)
    {
        if (_disposed || _sessionConfig is null || frame.Bytes.Length == 0 || frame.Width <= 0 || frame.Height <= 0 || frame.Stride <= 0)
        {
            return;
        }

        RoiMetadata? metadata;
        lock (_enhancementMetadataByPts)
        {
            if (!_enhancementMetadataByPts.Remove(frame.PresentationTimeUs, out metadata))
            {
                metadata = TakeNearestEnhancementMetadata(frame.PresentationTimeUs);
            }
        }

        if (metadata is null)
        {
            return;
        }

        var basePresentationTimeUs = _lastBasePresentationTimeUs;
        if (basePresentationTimeUs > 0 && Math.Abs(frame.PresentationTimeUs - basePresentationTimeUs) > 80_000)
        {
            return;
        }

        var overlayRect = MapRoiToBaseRect(metadata, _sessionConfig.Width, _sessionConfig.Height);
        if (overlayRect.Right <= overlayRect.Left || overlayRect.Bottom <= overlayRect.Top)
        {
            return;
        }

        _renderer.RenderCpuArgbOverlayFrame(
            frame.Bytes,
            frame.Width,
            frame.Height,
            frame.Stride,
            _sessionConfig.Width,
            _sessionConfig.Height,
            overlayRect);
    }

    private RoiMetadata? TakeNearestEnhancementMetadata(long presentationTimeUs)
    {
        if (_enhancementMetadataByPts.Count == 0)
        {
            return null;
        }

        var nearest = _enhancementMetadataByPts
            .OrderBy(entry => Math.Abs(entry.Key - presentationTimeUs))
            .FirstOrDefault();
        if (nearest.Equals(default(KeyValuePair<long, RoiMetadata>)))
        {
            return null;
        }

        if (Math.Abs(nearest.Key - presentationTimeUs) > 120_000)
        {
            return null;
        }

        _enhancementMetadataByPts.Remove(nearest.Key);
        return nearest.Value;
    }

    private static RawRect MapRoiToBaseRect(RoiMetadata metadata, int baseWidth, int baseHeight)
    {
        var screenWidth = Math.Max(1, metadata.ScreenWidth);
        var screenHeight = Math.Max(1, metadata.ScreenHeight);
        var left = (int)Math.Round(metadata.X / (double)screenWidth * baseWidth);
        var top = (int)Math.Round(metadata.Y / (double)screenHeight * baseHeight);
        var right = (int)Math.Round((metadata.X + metadata.Width) / (double)screenWidth * baseWidth);
        var bottom = (int)Math.Round((metadata.Y + metadata.Height) / (double)screenHeight * baseHeight);

        left = Math.Clamp(left, 0, Math.Max(0, baseWidth - 1));
        top = Math.Clamp(top, 0, Math.Max(0, baseHeight - 1));
        right = Math.Clamp(right, left + 1, Math.Max(left + 1, baseWidth));
        bottom = Math.Clamp(bottom, top + 1, Math.Max(top + 1, baseHeight));
        return new RawRect(left, top, right, bottom);
    }

    private async Task PaceFrameIfEarlyAsync(long sampleTimeHns, CancellationToken token)
    {
        if (!_ultraLowLatencyMode || sampleTimeHns <= 0)
        {
            return;
        }

        var nowTicks = Stopwatch.GetTimestamp();
        if (_pacingBaseSampleTimeHns is null)
        {
            _pacingBaseSampleTimeHns = sampleTimeHns;
            _pacingBaseTicks = nowTicks;
            return;
        }

        var targetTicks = _pacingBaseTicks +
            (long)((sampleTimeHns - _pacingBaseSampleTimeHns.Value) * (double)Stopwatch.Frequency / 10_000_000.0);
        var waitTicks = targetTicks - nowTicks;
        if (waitTicks <= 0)
        {
            return;
        }

        var waitMs = waitTicks * 1000.0 / Stopwatch.Frequency;
        if (waitMs <= 0.15 || waitMs > 4.0)
        {
            return;
        }

        await Task.Delay(TimeSpan.FromMilliseconds(waitMs), token);
    }

    private static TimeSpan ElapsedSince(long thenTicks, long nowTicks)
    {
        if (thenTicks == 0 || nowTicks <= thenTicks)
        {
            return TimeSpan.Zero;
        }

        var seconds = (nowTicks - thenTicks) / (double)Stopwatch.Frequency;
        return TimeSpan.FromSeconds(seconds);
    }

    private sealed class MediaFoundationDecoderSession : IDisposable
    {
        private readonly D3D11SwapChainRenderer _renderer;
        private readonly SessionConfig _config;
        private readonly bool _preferHardware;
        private readonly IMFTransform _decoder;
        private readonly IMFDXGIDeviceManager? _deviceManager;
        private readonly int _inputStreamId = 0;
        private readonly int _outputStreamId = 0;
        private OutputMode _outputMode;
        private OutputStreamInfo _outputStreamInfo;
        private bool _disposed;

        public MediaFoundationDecoderSession(
            D3D11SwapChainRenderer renderer,
            SessionConfig config,
            HardwareDecodeMode mode,
            bool ultraLowLatencyMode,
            bool forceSoftwareSafePath)
        {
            _renderer = renderer;
            _config = config;
            _preferHardware = !forceSoftwareSafePath && mode != HardwareDecodeMode.Disabled;

            _deviceManager = _preferHardware ? CreateDeviceManager(renderer.Device) : null;
            _decoder = CreateDecoder(config);
            if (_deviceManager is not null)
            {
                _decoder.ProcessMessage(TMessageType.MessageSetD3DManager, (UIntPtr)_deviceManager.NativePointer);
            }
            ConfigureInputType(config);
            _outputMode = ConfigureOutputType(
                config,
                ultraLowLatencyMode,
                preferCpuStableOutput: true);
            _outputStreamInfo = _decoder.GetOutputStreamInfo(_outputStreamId);

            _decoder.ProcessMessage(TMessageType.MessageNotifyBeginStreaming, UIntPtr.Zero);
            _decoder.ProcessMessage(TMessageType.MessageNotifyStartOfStream, UIntPtr.Zero);
        }

        public void ProcessAccessUnit(byte[] bytes, bool isKeyFrame, long sampleTimeHns, long sampleDurationHns)
        {
            if (_disposed)
            {
                return;
            }

            using var sample = CreateInputSample(bytes, sampleTimeHns, sampleDurationHns);
            var inputStatus = (InputStatusFlags)_decoder.GetInputStatus(_inputStreamId);
            if ((inputStatus & InputStatusFlags.InputStatusAcceptData) == 0)
            {
                DrainOutputs();
            }

            _decoder.ProcessInput(_inputStreamId, sample, 0);
            DrainOutputs();
        }

        public void Flush()
        {
            if (_disposed)
            {
                return;
            }

            _decoder.ProcessMessage(TMessageType.MessageCommandFlush, UIntPtr.Zero);
        }

        public void Dispose()
        {
            if (_disposed)
            {
                return;
            }

            _disposed = true;
            try
            {
                _decoder.ProcessMessage(TMessageType.MessageNotifyEndOfStream, UIntPtr.Zero);
                _decoder.ProcessMessage(TMessageType.MessageNotifyEndStreaming, UIntPtr.Zero);
            }
            catch
            {
            }

            _decoder.Dispose();
            _deviceManager?.Dispose();
        }

        private IMFTransform CreateDecoder(SessionConfig config)
        {
            var inputType = new RegisterTypeInfo
            {
                GuidMajorType = MediaTypeGuids.Video,
                GuidSubtype = config.Codec.Contains("hevc", StringComparison.OrdinalIgnoreCase)
                    ? VideoFormatGuids.HevcEs
                    : VideoFormatGuids.H264Es,
            };

            var flags = _preferHardware
                ? (uint)(EnumFlag.EnumFlagHardware | EnumFlag.EnumFlagSortandfilter)
                : (uint)(EnumFlag.EnumFlagAll | EnumFlag.EnumFlagSortandfilter);
            var activates = MediaFactory.MFTEnumEx(TransformCategoryGuids.VideoDecoder, flags, inputType, null);
            foreach (var activate in activates)
            {
                try
                {
                    return activate.ActivateObject<IMFTransform>();
                }
                catch
                {
                }
                finally
                {
                    activate.Dispose();
                }
            }

            if (_preferHardware)
            {
                var fallback = MediaFactory.MFTEnumEx(TransformCategoryGuids.VideoDecoder, (uint)(EnumFlag.EnumFlagAll | EnumFlag.EnumFlagSortandfilter), inputType, null);
                foreach (var activate in fallback)
                {
                    try
                    {
                        return activate.ActivateObject<IMFTransform>();
                    }
                    catch
                    {
                    }
                    finally
                    {
                        activate.Dispose();
                    }
                }
            }

            throw new InvalidOperationException("Media Foundation decoder not found");
        }

        private void ConfigureInputType(SessionConfig config)
        {
            using var inputType = MediaFactory.MFCreateMediaType();
            inputType.Set(MediaTypeAttributeKeys.MajorType, MediaTypeGuids.Video).CheckError();
            inputType.Set(
                MediaTypeAttributeKeys.Subtype,
                config.Codec.Contains("hevc", StringComparison.OrdinalIgnoreCase)
                    ? VideoFormatGuids.HevcEs
                    : VideoFormatGuids.H264Es).CheckError();
            MediaFactory.MFSetAttributeSize(inputType, MediaTypeAttributeKeys.FrameSize, (uint)config.Width, (uint)config.Height).CheckError();
            MediaFactory.MFSetAttributeSize(inputType, MediaTypeAttributeKeys.FrameRate, (uint)Math.Max(1, config.Fps), 1).CheckError();
            inputType.Set(MediaTypeAttributeKeys.AvgBitrate, (uint)Math.Max(1, config.Bitrate)).CheckError();
            inputType.Set(MediaTypeAttributeKeys.InterlaceMode, 2u).CheckError();
            inputType.Set(MediaTypeAttributeKeys.AllSamplesIndependent, false).CheckError();
            _decoder.SetInputType(_inputStreamId, inputType, 0);
        }

        private OutputMode ConfigureOutputType(SessionConfig config, bool ultraLowLatencyMode, bool preferCpuStableOutput)
        {
            var preferredSubtypes = preferCpuStableOutput
                ? new[] { VideoFormatGuids.Rgb32, VideoFormatGuids.Argb32, VideoFormatGuids.NV12 }
                : new[] { VideoFormatGuids.NV12, VideoFormatGuids.Rgb32, VideoFormatGuids.Argb32 };

            foreach (var subtype in preferredSubtypes)
            {
                if (TryConfigureOutputTypeFromAvailableTypes(subtype, ultraLowLatencyMode))
                {
                    return subtype == VideoFormatGuids.NV12 ? OutputMode.GpuNv12 : OutputMode.CpuArgb;
                }
            }

            throw new InvalidOperationException("Failed to configure Media Foundation decoder output type");
        }

        private bool TryConfigureOutputTypeFromAvailableTypes(Guid subtype, bool ultraLowLatencyMode)
        {
            for (var index = 0; index < 32; index++)
            {
                IMFMediaType? availableType = null;
                try
                {
                    availableType = _decoder.GetOutputAvailableType(_outputStreamId, index);
                    var availableSubtype = availableType.GetGUID(MediaTypeAttributeKeys.Subtype);
                    if (availableSubtype != subtype)
                    {
                        continue;
                    }

                    if (ultraLowLatencyMode &&
                        (subtype == VideoFormatGuids.Argb32 || subtype == VideoFormatGuids.Rgb32))
                    {
                        TrySetDefaultStride(availableType, _config.Width * 4);
                    }

                    _decoder.SetOutputType(_outputStreamId, availableType, 0);
                    return true;
                }
                catch
                {
                    if (availableType is null)
                    {
                        break;
                    }
                }
                finally
                {
                    availableType?.Dispose();
                }
            }

            return false;
        }

        private static void TrySetDefaultStride(IMFMediaType mediaType, int stride)
        {
            try
            {
                mediaType.Set(MediaTypeAttributeKeys.DefaultStride, (uint)Math.Max(0, stride)).CheckError();
            }
            catch
            {
            }
        }

        private void DrainOutputs()
        {
            while (true)
            {
                IMFSample? providedSample = null;
                var outputBuffer = new OutputDataBuffer
                {
                    StreamID = _outputStreamId,
                };

                try
                {
                    if ((_outputStreamInfo.Flags & (int)OutputStreamInfoFlags.OutputStreamProvidesSamples) == 0 &&
                        (_outputStreamInfo.Flags & (int)OutputStreamInfoFlags.OutputStreamCanProvideSamples) == 0 &&
                        _outputMode == OutputMode.CpuArgb)
                    {
                        providedSample = MediaFactory.MFCreateSample();
                        using var mediaBuffer = MediaFactory.MFCreateMemoryBuffer(Math.Max(_outputStreamInfo.Size, _config.Width * _config.Height * 4));
                        providedSample.AddBuffer(mediaBuffer);
                        outputBuffer.Sample = providedSample;
                    }

                    var result = _decoder.ProcessOutput(ProcessOutputFlags.None, 1, ref outputBuffer, out _);
                    if (result == Vortice.MediaFoundation.ResultCode.TransformNeedMoreInput)
                    {
                        return;
                    }
                    if (result == Vortice.MediaFoundation.ResultCode.TransformStreamChange)
                    {
                        _outputMode = ConfigureOutputType(_config, ultraLowLatencyMode: true, preferCpuStableOutput: true);
                        _outputStreamInfo = _decoder.GetOutputStreamInfo(_outputStreamId);
                        continue;
                    }
                    result.CheckError();

                    using var sample = outputBuffer.Sample;
                    if (sample is null)
                    {
                        return;
                    }

                    RenderSample(sample);
                }
                finally
                {
                    providedSample?.Dispose();
                }
            }
        }

        private void RenderSample(IMFSample sample)
        {
            if (_outputMode == OutputMode.GpuNv12 && TryRenderDxgiSample(sample))
            {
                return;
            }

            if (_outputMode == OutputMode.GpuNv12)
            {
                throw new InvalidOperationException("Media Foundation returned NV12 sample without DXGI surface");
            }

            RenderCpuSample(sample);
        }

        private bool TryRenderDxgiSample(IMFSample sample)
        {
            try
            {
                using var mediaBuffer = sample.ConvertToContiguousBuffer();
                using var dxgiBuffer = mediaBuffer.QueryInterfaceOrNull<IMFDXGIBuffer>();
                if (dxgiBuffer is null)
                {
                    return false;
                }

                var resourcePointer = dxgiBuffer.GetResource(typeof(ID3D11Texture2D).GUID);
                if (resourcePointer == IntPtr.Zero)
                {
                    return false;
                }

                using var texture = new ID3D11Texture2D(resourcePointer);
                _renderer.RenderGpuTexture(texture, dxgiBuffer.SubresourceIndex, Format.NV12, _config.Width, _config.Height);
                return true;
            }
            catch
            {
                return false;
            }
        }

        private void RenderCpuSample(IMFSample sample)
        {
            using var buffer = sample.ConvertToContiguousBuffer();
            using var buffer2D = buffer.QueryInterfaceOrNull<IMF2DBuffer>();
            if (buffer2D is not null)
            {
                buffer2D.Lock2D(out var scanline0, out var pitch);
                try
                {
                    if (scanline0 == IntPtr.Zero)
                    {
                        return;
                    }

                    _renderer.RenderCpuArgbFrame(scanline0, _config.Width, _config.Height, Math.Abs(pitch));
                    return;
                }
                finally
                {
                    buffer2D.Unlock2D();
                }
            }

            buffer.Lock(out var data, out _, out var currentLength);
            try
            {
                if (data == IntPtr.Zero || currentLength <= 0)
                {
                    return;
                }

                _renderer.RenderCpuArgbFrame(data, _config.Width, _config.Height, _config.Width * 4);
            }
            finally
            {
                buffer.Unlock();
            }
        }

        private IMFSample CreateInputSample(byte[] bytes, long sampleTimeHns, long sampleDurationHns)
        {
            var sample = MediaFactory.MFCreateSample();
            var buffer = MediaFactory.MFCreateMemoryBuffer(bytes.Length);
            try
            {
                buffer.Lock(out var destination, out _, out _);
                try
                {
                    System.Runtime.InteropServices.Marshal.Copy(bytes, 0, destination, bytes.Length);
                    buffer.CurrentLength = bytes.Length;
                }
                finally
                {
                    buffer.Unlock();
                }

                sample.AddBuffer(buffer);
                sample.SampleTime = sampleTimeHns;
                sample.SampleDuration = sampleDurationHns;
                return sample;
            }
            catch
            {
                buffer.Dispose();
                sample.Dispose();
                throw;
            }
        }

        private static IMFDXGIDeviceManager CreateDeviceManager(ID3D11Device device)
        {
            var manager = MediaFactory.MFCreateDXGIDeviceManager();
            manager.ResetDevice(device).CheckError();
            return manager;
        }

        private enum OutputMode
        {
            GpuNv12,
            CpuArgb,
        }
    }

    private sealed class MediaFoundationEnhancementDecoderSession : IDisposable
    {
        private readonly SessionConfig _config;
        private readonly HardwareDecodeMode _mode;
        private readonly bool _forceSoftwareSafePath;
        private readonly Action<DecodedEnhancementFrame> _onFrameDecoded;
        private readonly IMFTransform _decoder;
        private readonly int _inputStreamId = 0;
        private readonly int _outputStreamId = 0;
        private OutputStreamInfo _outputStreamInfo;
        private bool _disposed;

        public MediaFoundationEnhancementDecoderSession(
            SessionConfig config,
            HardwareDecodeMode mode,
            bool forceSoftwareSafePath,
            Action<DecodedEnhancementFrame> onFrameDecoded)
        {
            _config = config;
            _mode = mode;
            _forceSoftwareSafePath = forceSoftwareSafePath;
            _onFrameDecoded = onFrameDecoded;
            _decoder = CreateDecoder();
            ConfigureInputType();
            ConfigureOutputType();
            _outputStreamInfo = _decoder.GetOutputStreamInfo(_outputStreamId);
            _decoder.ProcessMessage(TMessageType.MessageNotifyBeginStreaming, UIntPtr.Zero);
            _decoder.ProcessMessage(TMessageType.MessageNotifyStartOfStream, UIntPtr.Zero);
        }

        public void ProcessAccessUnit(byte[] bytes, bool isKeyFrame, long sampleTimeHns, long sampleDurationHns)
        {
            if (_disposed)
            {
                return;
            }

            using var sample = CreateInputSample(bytes, sampleTimeHns, sampleDurationHns);
            var inputStatus = (InputStatusFlags)_decoder.GetInputStatus(_inputStreamId);
            if ((inputStatus & InputStatusFlags.InputStatusAcceptData) == 0)
            {
                DrainOutputs();
            }

            _decoder.ProcessInput(_inputStreamId, sample, 0);
            DrainOutputs();
        }

        public void Flush()
        {
            if (_disposed)
            {
                return;
            }

            _decoder.ProcessMessage(TMessageType.MessageCommandFlush, UIntPtr.Zero);
        }

        public void Dispose()
        {
            if (_disposed)
            {
                return;
            }

            _disposed = true;
            try
            {
                _decoder.ProcessMessage(TMessageType.MessageNotifyEndOfStream, UIntPtr.Zero);
                _decoder.ProcessMessage(TMessageType.MessageNotifyEndStreaming, UIntPtr.Zero);
            }
            catch
            {
            }

            _decoder.Dispose();
        }

        private IMFTransform CreateDecoder()
        {
            var codecMime = string.IsNullOrWhiteSpace(_config.EnhancementCodec) ? _config.Codec : _config.EnhancementCodec!;
            var inputType = new RegisterTypeInfo
            {
                GuidMajorType = MediaTypeGuids.Video,
                GuidSubtype = codecMime.Contains("hevc", StringComparison.OrdinalIgnoreCase)
                    ? VideoFormatGuids.HevcEs
                    : VideoFormatGuids.H264Es,
            };

            var preferHardware = !_forceSoftwareSafePath && _mode != HardwareDecodeMode.Disabled;
            var flags = preferHardware
                ? (uint)(EnumFlag.EnumFlagHardware | EnumFlag.EnumFlagSortandfilter)
                : (uint)(EnumFlag.EnumFlagAll | EnumFlag.EnumFlagSortandfilter);
            var activates = MediaFactory.MFTEnumEx(TransformCategoryGuids.VideoDecoder, flags, inputType, null);
            foreach (var activate in activates)
            {
                try
                {
                    return activate.ActivateObject<IMFTransform>();
                }
                catch
                {
                }
                finally
                {
                    activate.Dispose();
                }
            }

            throw new InvalidOperationException("Media Foundation enhancement decoder not found");
        }

        private void ConfigureInputType()
        {
            using var inputType = MediaFactory.MFCreateMediaType();
            inputType.Set(MediaTypeAttributeKeys.MajorType, MediaTypeGuids.Video).CheckError();
            inputType.Set(
                MediaTypeAttributeKeys.Subtype,
                (_config.EnhancementCodec ?? _config.Codec).Contains("hevc", StringComparison.OrdinalIgnoreCase)
                    ? VideoFormatGuids.HevcEs
                    : VideoFormatGuids.H264Es).CheckError();
            MediaFactory.MFSetAttributeSize(inputType, MediaTypeAttributeKeys.FrameSize, (uint)GetWidth(), (uint)GetHeight()).CheckError();
            MediaFactory.MFSetAttributeSize(inputType, MediaTypeAttributeKeys.FrameRate, (uint)Math.Max(1, _config.Fps), 1).CheckError();
            inputType.Set(MediaTypeAttributeKeys.AvgBitrate, (uint)Math.Max(1, _config.Bitrate)).CheckError();
            inputType.Set(MediaTypeAttributeKeys.InterlaceMode, 2u).CheckError();
            inputType.Set(MediaTypeAttributeKeys.AllSamplesIndependent, false).CheckError();
            _decoder.SetInputType(_inputStreamId, inputType, 0);
        }

        private void ConfigureOutputType()
        {
            foreach (var subtype in new[] { VideoFormatGuids.Rgb32, VideoFormatGuids.Argb32 })
            {
                for (var index = 0; index < 32; index++)
                {
                    IMFMediaType? availableType = null;
                    try
                    {
                        availableType = _decoder.GetOutputAvailableType(_outputStreamId, index);
                        if (availableType.GetGUID(MediaTypeAttributeKeys.Subtype) != subtype)
                        {
                            continue;
                        }

                        TrySetDefaultStride(availableType, GetWidth() * 4);
                        _decoder.SetOutputType(_outputStreamId, availableType, 0);
                        return;
                    }
                    catch
                    {
                        if (availableType is null)
                        {
                            break;
                        }
                    }
                    finally
                    {
                        availableType?.Dispose();
                    }
                }
            }

            throw new InvalidOperationException("Failed to configure enhancement decoder output type");
        }

        private void DrainOutputs()
        {
            while (true)
            {
                IMFSample? providedSample = null;
                var outputBuffer = new OutputDataBuffer
                {
                    StreamID = _outputStreamId,
                };

                try
                {
                    if ((_outputStreamInfo.Flags & (int)OutputStreamInfoFlags.OutputStreamProvidesSamples) == 0 &&
                        (_outputStreamInfo.Flags & (int)OutputStreamInfoFlags.OutputStreamCanProvideSamples) == 0)
                    {
                        providedSample = MediaFactory.MFCreateSample();
                        using var mediaBuffer = MediaFactory.MFCreateMemoryBuffer(Math.Max(_outputStreamInfo.Size, GetWidth() * GetHeight() * 4));
                        providedSample.AddBuffer(mediaBuffer);
                        outputBuffer.Sample = providedSample;
                    }

                    var result = _decoder.ProcessOutput(ProcessOutputFlags.None, 1, ref outputBuffer, out _);
                    if (result == Vortice.MediaFoundation.ResultCode.TransformNeedMoreInput)
                    {
                        return;
                    }
                    if (result == Vortice.MediaFoundation.ResultCode.TransformStreamChange)
                    {
                        ConfigureOutputType();
                        _outputStreamInfo = _decoder.GetOutputStreamInfo(_outputStreamId);
                        continue;
                    }
                    result.CheckError();

                    using var sample = outputBuffer.Sample;
                    if (sample is null)
                    {
                        return;
                    }

                    var decoded = ConvertSample(sample);
                    if (decoded is not null)
                    {
                        _onFrameDecoded(decoded);
                    }
                }
                finally
                {
                    providedSample?.Dispose();
                }
            }
        }

        private DecodedEnhancementFrame? ConvertSample(IMFSample sample)
        {
            using var buffer = sample.ConvertToContiguousBuffer();
            using var buffer2D = buffer.QueryInterfaceOrNull<IMF2DBuffer>();
            if (buffer2D is not null)
            {
                buffer2D.Lock2D(out var scanline0, out var pitch);
                try
                {
                    if (scanline0 == IntPtr.Zero)
                    {
                        return null;
                    }

                    var width = GetWidth();
                    var height = GetHeight();
                    var stride = Math.Abs(pitch);
                    var bytes = new byte[stride * height];
                    Marshal.Copy(scanline0, bytes, 0, bytes.Length);
                    return new DecodedEnhancementFrame(bytes, width, height, stride, sample.SampleTime / 10L);
                }
                finally
                {
                    buffer2D.Unlock2D();
                }
            }

            buffer.Lock(out var data, out _, out var currentLength);
            try
            {
                if (data == IntPtr.Zero || currentLength <= 0)
                {
                    return null;
                }

                var bytes = new byte[currentLength];
                Marshal.Copy(data, bytes, 0, currentLength);
                return new DecodedEnhancementFrame(bytes, GetWidth(), GetHeight(), GetWidth() * 4, sample.SampleTime / 10L);
            }
            finally
            {
                buffer.Unlock();
            }
        }

        private IMFSample CreateInputSample(byte[] bytes, long sampleTimeHns, long sampleDurationHns)
        {
            var sample = MediaFactory.MFCreateSample();
            var buffer = MediaFactory.MFCreateMemoryBuffer(bytes.Length);
            try
            {
                buffer.Lock(out var destination, out _, out _);
                try
                {
                    Marshal.Copy(bytes, 0, destination, bytes.Length);
                    buffer.CurrentLength = bytes.Length;
                }
                finally
                {
                    buffer.Unlock();
                }

                sample.AddBuffer(buffer);
                sample.SampleTime = sampleTimeHns;
                sample.SampleDuration = sampleDurationHns;
                return sample;
            }
            catch
            {
                buffer.Dispose();
                sample.Dispose();
                throw;
            }
        }

        private int GetWidth() => Math.Max(16, _config.EnhancementMaxWidth > 0 ? _config.EnhancementMaxWidth : _config.Width);
        private int GetHeight() => Math.Max(16, _config.EnhancementMaxHeight > 0 ? _config.EnhancementMaxHeight : _config.Height);

        private static void TrySetDefaultStride(IMFMediaType mediaType, int stride)
        {
            try
            {
                mediaType.Set(MediaTypeAttributeKeys.DefaultStride, (uint)Math.Max(0, stride)).CheckError();
            }
            catch
            {
            }
        }
    }

    private sealed record DecodedEnhancementFrame(byte[] Bytes, int Width, int Height, int Stride, long PresentationTimeUs);
}
