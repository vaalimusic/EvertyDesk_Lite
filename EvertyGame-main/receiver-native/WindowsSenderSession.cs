using SharpGen.Runtime;
using System.Buffers;
using System.Diagnostics;
using System.Drawing.Imaging;
using System.Globalization;
using System.Net;
using System.Net.Sockets;
using System.Runtime.InteropServices;
using System.Text.Json;
using NAudio.Wave;
using Vortice.Direct3D;
using Vortice.Direct3D11;
using Vortice.DXGI;
using Vortice.MediaFoundation;

namespace ReceiverNative;

internal sealed record WindowsCaptureTargetInfo(
    string DeviceName,
    string UiLabel,
    Rectangle Bounds);

internal sealed record WindowsSenderSessionSnapshot
{
    public bool Sending { get; init; }
    public string Status { get; init; } = "Idle";
    public string Preset { get; init; } = "-";
    public string EncoderPath { get; init; } = "-";
    public string AutoEncoderSelected { get; init; } = "-";
    public string CaptureTarget { get; init; } = "-";
    public string Codec { get; init; } = "-";
    public string Resolution { get; init; } = "-";
    public int TargetFps { get; init; }
    public double BitrateMbps { get; init; }
    public string RemoteEndpoint { get; init; } = "-";
    public long PacketsSent { get; init; }
    public long SessionConfigPackets { get; init; }
    public long CodecConfigPackets { get; init; }
    public long VideoPackets { get; init; }
    public long AudioPackets { get; init; }
    public long ControlPacketsSent { get; init; }
    public long ControlPacketsReceived { get; init; }
    public long FramesCaptured { get; init; }
    public long FramesEncoded { get; init; }
    public long FramesDropped { get; init; }
    public int CaptureFps { get; init; }
    public int SubmitFps { get; init; }
    public int EncodeFps { get; init; }
    public long NativeDxgiTimeouts { get; init; }
    public long NativePacedSkips { get; init; }
    public string AudioStatus { get; init; } = "-";
    public string GamepadStatus { get; init; } = "-";
    public string GamepadInput { get; init; } = "-";
    public string LastControlKind { get; init; } = "-";
    public string ReceiverPressure { get; init; } = "-";
    public int ReceiverDecodeFps { get; init; } = 0;
    public long ReceiverQueueDrops { get; init; }
    public int ReceiverDecodeDeltaMs { get; init; } = -1;
    public int ReceiverPresentDeltaMs { get; init; } = -1;
    public int PulseToAndroidEstimateMs { get; init; } = -1;
    public int InputToAndroidEstimateMs { get; init; } = -1;
    public int ReceiverFeedbackAgeMs { get; init; } = -1;
    public bool AdaptiveEnabled { get; init; }
    public int AdaptiveStep { get; init; }
    public string LastEncoderError { get; init; } = "-";
    public string NativeStageStats { get; init; } = "-";
}

internal sealed class WindowsSenderSession : IDisposable
{
    private const int KeyFrameRequestCooldownMs = 1200;
    private const int KeyFrameRequestStartupGraceMs = 1500;
    private const int AdaptiveChangeCooldownMs = 3500;
    private const int UdpConnectionResetIoControlCode = unchecked((int)0x9800000C);

    internal static readonly FeatureLevel[] PreferredFeatureLevels =
    {
        FeatureLevel.Level_11_1,
        FeatureLevel.Level_11_0,
        FeatureLevel.Level_10_1,
        FeatureLevel.Level_10_0,
    };

    private readonly object _sync = new();
    private readonly object _udpSendSync = new();
    private readonly EvrtPacketizer _packetizer = new();
    private readonly WindowsRemoteInputInjector _inputInjector = new();
    private readonly WindowsVirtualGamepadInjector _gamepadInjector = new();
    private readonly LatencyPulseOverlayController _latencyPulseOverlay = new();
    private readonly Queue<long> _captureTicks = new();
    private readonly Queue<long> _submitTicks = new();
    private readonly Queue<long> _encodeTicks = new();
    private WindowsSenderPresetSpec _baseSenderSpec = WindowsSenderPreset.Game.ToSpec();
    private WindowsSenderPresetSpec _senderSpec = WindowsSenderPreset.Game.ToSpec();
    private WindowsSenderEncoderBackend _encoderBackend = WindowsSenderEncoderBackend.Auto;
    private WindowsVideoCodec _selectedCodec = WindowsVideoCodec.H265Hevc;
    private bool _audioEnabled = true;
    private bool _captureCursorInStream;
    private bool _latencyPulseFlashEnabled;
    private bool _adaptiveEnabled = true;
    private double _nativeAcquireWaitMsEwma;
    private double _nativeAcquireProcessMsEwma;
    private double _nativeEncodeCallMsEwma;
    private double _nativeDrainPacketizeMsEwma;
    private static SenderCapabilityProbeResult? s_capabilityProbe;
    private static readonly object s_capabilityProbeSync = new();

    private WindowsSenderSessionSnapshot _snapshot = new();
    private UdpClient? _udpClient;
    private CancellationTokenSource? _cts;
    private Task? _captureTask;
    private Task? _controlTask;
    private Task? _audioTask;
    private Task? _relayTask;
    private RelayTransportRoute? _relayRoute;
    private Rectangle _selectedMonitorBounds;
    private int _forceEncoderRestartRequested;
    private long _lastAcceptedControlSeq;
    private long _lastEncoderStartTicks;
    private long _lastKeyFrameRestartTicks;
    private long _lastControlPacketReceivedTicks;
    private long _lastReceiverFeedbackTicks;
    private long _nextLatencyPulseId;
    private int _nextFrameId;
    private byte[]? _lastCodecConfig;
    private PendingLatencyPulse? _pendingLatencyPulse;
    private long _sampleTimeHns;
    private long _sampleDurationHns;
    private int _adaptiveStep;
    private int _pendingAdaptiveStep = -1;
    private int _receiverStrainScore;
    private int _receiverRecoveryScore;
    private long _lastAdaptiveChangeTicks;
    private int _targetWidth;
    private int _targetHeight;
    private int _targetBitrateBps;
    private int _targetFps;
    private string _remoteHost = "127.0.0.1";
    private int _remotePort;
    private int _nextAudioFrameId;
    private long _lastControlLoopConnectionResetLogTicks;

    private enum FfmpegCaptureInputBackend
    {
        GdiGrab,
        DdaGrab,
    }

    internal sealed record SenderCapabilityProbeResult(
        bool FfmpegAvailable,
        bool HasNvidiaAdapter,
        bool HasIntelAdapter,
        bool NativeNvencAvc,
        bool NativeNvencHevc,
        bool NvencAvc,
        bool NvencHevc,
        bool NvencAv1,
        bool QuickSyncAvc,
        bool QuickSyncHevc,
        bool QuickSyncAv1,
        bool SoftwareAvc,
        bool SoftwareHevc,
        bool SoftwareAv1,
        bool MediaFoundationAvc,
        bool MediaFoundationHevc)
    {
        public string[] SupportedEncodeCodecs =>
            new[]
            {
                SupportsAdvertisedEncodeCodec(WindowsVideoCodec.Av1) ? WindowsVideoCodec.Av1.ToMimeType() : null,
                SupportsAdvertisedEncodeCodec(WindowsVideoCodec.H265Hevc) ? WindowsVideoCodec.H265Hevc.ToMimeType() : null,
                SupportsAdvertisedEncodeCodec(WindowsVideoCodec.H264Avc) ? WindowsVideoCodec.H264Avc.ToMimeType() : null,
            }
            .Where(static codec => !string.IsNullOrWhiteSpace(codec))
            .Cast<string>()
            .ToArray();

        public string[] SupportedBackends =>
            new[]
            {
                HasAnyNativeNvenc ? WindowsSenderEncoderBackend.NvidiaNvencNative.ToString() : null,
                HasAnyFfmpegNvenc ? WindowsSenderEncoderBackend.NvidiaNvenc.ToString() : null,
                HasAnyQuickSync ? WindowsSenderEncoderBackend.IntelQuickSync.ToString() : null,
                HasAnyMediaFoundation ? WindowsSenderEncoderBackend.MediaFoundation.ToString() : null,
                WindowsSenderEncoderBackend.FfmpegSoftware.ToString(),
            }
            .Where(static backend => !string.IsNullOrWhiteSpace(backend))
            .Cast<string>()
            .ToArray();

        public bool HasAnyNativeNvenc => NativeNvencAvc || NativeNvencHevc;
        public bool HasAnyFfmpegNvenc => NvencAvc || NvencHevc || NvencAv1;
        public bool HasAnyNvenc => HasAnyNativeNvenc || HasAnyFfmpegNvenc;
        public bool HasAnyQuickSync => QuickSyncAvc || QuickSyncHevc || QuickSyncAv1;
        public bool HasAnyMediaFoundation => MediaFoundationAvc || MediaFoundationHevc;

        public bool SupportsAdvertisedEncodeCodec(WindowsVideoCodec codec) =>
            codec switch
            {
                WindowsVideoCodec.H264Avc => NativeNvencAvc || NvencAvc || QuickSyncAvc || MediaFoundationAvc || SoftwareAvc,
                WindowsVideoCodec.H265Hevc => NativeNvencHevc || NvencHevc || QuickSyncHevc || SoftwareHevc,
                WindowsVideoCodec.Av1 => NvencAv1 || QuickSyncAv1 || SoftwareAv1,
                _ => false,
            };

        public bool SupportsCodec(WindowsVideoCodec codec) =>
            codec switch
            {
                WindowsVideoCodec.H264Avc => NativeNvencAvc || NvencAvc || QuickSyncAvc || MediaFoundationAvc || SoftwareAvc,
                WindowsVideoCodec.H265Hevc => NativeNvencHevc || NvencHevc || QuickSyncHevc || MediaFoundationHevc || SoftwareHevc,
                WindowsVideoCodec.Av1 => NvencAv1 || QuickSyncAv1 || SoftwareAv1,
                _ => false,
            };

        public bool SupportsNativeNvenc(WindowsVideoCodec codec) =>
            codec switch
            {
                WindowsVideoCodec.H264Avc => NativeNvencAvc,
                WindowsVideoCodec.H265Hevc => NativeNvencHevc,
                _ => false,
            };
    }

    private static readonly EncoderPlan[] EncoderPlans =
    {
        new("RGB32 + hardware transforms", VideoFormatGuids.Rgb32, true),
        new("RGB32 + software transforms", VideoFormatGuids.Rgb32, false),
        new("ARGB32 + hardware transforms", VideoFormatGuids.Argb32, true),
        new("ARGB32 + software transforms", VideoFormatGuids.Argb32, false),
    };

    private sealed record PendingLatencyPulse(
        long PulseId,
        long InputSeq,
        string Source,
        long TriggerTicks);

    private string GetTransportTargetLabel() => _relayRoute?.DisplayText ?? $"{_remoteHost}:{_remotePort}";

    public WindowsSenderSessionSnapshot GetSnapshot()
    {
        lock (_sync)
        {
            var feedbackAgeMs = _lastReceiverFeedbackTicks <= 0
                ? -1
                : (int)Math.Clamp(
                    Math.Round(Stopwatch.GetElapsedTime(_lastReceiverFeedbackTicks, Stopwatch.GetTimestamp()).TotalMilliseconds),
                    0,
                    int.MaxValue);
            return _snapshot with { ReceiverFeedbackAgeMs = feedbackAgeMs };
        }
    }

    public static IReadOnlyList<WindowsCaptureTargetInfo> GetCaptureTargets()
    {
        return Screen.AllScreens
            .Select(
                screen => new WindowsCaptureTargetInfo(
                    DeviceName: screen.DeviceName,
                    UiLabel: $"{screen.DeviceName} ({screen.Bounds.Width}x{screen.Bounds.Height})",
                    Bounds: screen.Bounds))
            .ToArray();
    }

    public void Start(
        string host,
        int port,
        string captureTargetDeviceName,
        WindowsSenderEncoderBackend encoderBackend,
        WindowsVideoCodec codec,
        WindowsSenderPresetSpec spec,
        bool audioEnabled,
        bool captureCursorInStream,
        bool latencyPulseFlashEnabled,
        bool adaptiveEnabled,
        RelayTransportRoute? relayRoute = null)
    {
        Stop();

        var target = ResolveCaptureTarget(captureTargetDeviceName);
        _sampleTimeHns = 0;
        _selectedMonitorBounds = target.Bounds;
        _encoderBackend = encoderBackend;
        _selectedCodec = codec;
        _baseSenderSpec = spec;
        _adaptiveStep = 0;
        _pendingAdaptiveStep = -1;
        _receiverStrainScore = 0;
        _receiverRecoveryScore = 0;
        _lastAdaptiveChangeTicks = 0;
        ApplySenderSpec(spec, target.Bounds.Size);
        _audioEnabled = audioEnabled;
        _captureCursorInStream = captureCursorInStream;
        _latencyPulseFlashEnabled = latencyPulseFlashEnabled;
        _adaptiveEnabled = adaptiveEnabled;
        _relayRoute = relayRoute;
        _remoteHost = host;
        _remotePort = port;
        _nextFrameId = 1;
        _lastCodecConfig = null;
        _forceEncoderRestartRequested = 0;
        _lastAcceptedControlSeq = 0;
        _lastEncoderStartTicks = 0;
        _lastKeyFrameRestartTicks = 0;
        _lastControlPacketReceivedTicks = 0;
        _lastReceiverFeedbackTicks = 0;
        _nextLatencyPulseId = 0;
        _pendingLatencyPulse = null;
        _captureTicks.Clear();
        _submitTicks.Clear();
        _encodeTicks.Clear();
        _inputInjector.ResetSession();
        _gamepadInjector.ResetSession();
        _nextAudioFrameId = 1;

        var udpClient = new UdpClient(0);
        udpClient.Client.ReceiveBufferSize = 256 * 1024;
        udpClient.Client.SendBufferSize = 256 * 1024;
        DisableUdpConnectionReset(udpClient.Client);
        udpClient.Connect(relayRoute?.RelayHost ?? host, relayRoute?.RelayPort ?? port);

        _udpClient = udpClient;
        _cts = new CancellationTokenSource();

        lock (_sync)
        {
            _snapshot = new WindowsSenderSessionSnapshot
            {
                Sending = true,
                Status = $"Starting sender to {GetTransportTargetLabel()}",
                Preset = _baseSenderSpec.UiLabel,
                EncoderPath = _encoderBackend.ToPathLabel(),
                CaptureTarget = target.UiLabel,
                Codec = _selectedCodec.ToUiLabel(),
                Resolution = $"{_targetWidth}x{_targetHeight}",
                TargetFps = _targetFps,
                BitrateMbps = _targetBitrateBps / 1_000_000.0,
                RemoteEndpoint = relayRoute?.DisplayText ?? $"{host}:{port}",
                AudioStatus = audioEnabled ? "Starting loopback..." : "Disabled",
                GamepadStatus = _gamepadInjector.Status,
                GamepadInput = _gamepadInjector.LastInputSummary,
                ReceiverPressure = "-",
                ReceiverDecodeFps = 0,
                ReceiverQueueDrops = 0,
                ReceiverDecodeDeltaMs = -1,
                ReceiverPresentDeltaMs = -1,
                AdaptiveEnabled = _adaptiveEnabled,
                AdaptiveStep = _adaptiveStep,
            };
        }

        var token = _cts.Token;
        SendRelayRegistration();
        _relayTask = relayRoute is not null
            ? Task.Run(() => RelayRegistrationLoopAsync(token), token)
            : null;
        _captureTask = Task.Run(() => CaptureLoopAsync(target, token), token);
        _controlTask = Task.Run(() => ControlLoopAsync(token), token);
        _audioTask = _audioEnabled
            ? Task.Run(() => AudioLoopAsync(token), token)
            : null;
    }

    public void Stop()
    {
        var cts = _cts;
        var udpClient = _udpClient;
        var captureTask = _captureTask;
        var controlTask = _controlTask;
        var audioTask = _audioTask;
        var relayTask = _relayTask;

        _cts = null;
        _udpClient = null;
        _captureTask = null;
        _controlTask = null;
        _audioTask = null;
        _relayTask = null;
        _relayRoute = null;

        if (cts is not null)
        {
            cts.Cancel();
        }

        udpClient?.Dispose();

        try
        {
            captureTask?.Wait(500);
            controlTask?.Wait(500);
            audioTask?.Wait(500);
            relayTask?.Wait(500);
        }
        catch
        {
        }

        _inputInjector.ResetSession();
        _gamepadInjector.ResetSession();
        _pendingLatencyPulse = null;
        _latencyPulseOverlay.HidePulse();

        lock (_sync)
        {
            _snapshot = _snapshot with
            {
                Sending = false,
                Status = "Idle",
                AudioStatus = "-",
                GamepadStatus = _gamepadInjector.Status,
                GamepadInput = _gamepadInjector.LastInputSummary,
            };
        }
    }

    public void Dispose()
    {
        Stop();
        _gamepadInjector.Dispose();
        _latencyPulseOverlay.Dispose();
    }

    private async Task ControlLoopAsync(CancellationToken cancellationToken)
    {
        TryRaiseCurrentThreadPriority(ThreadPriority.AboveNormal);

        while (!cancellationToken.IsCancellationRequested)
        {
            UdpReceiveResult result;
            try
            {
                result = await _udpClient!.ReceiveAsync(cancellationToken);
            }
            catch (OperationCanceledException)
            {
                return;
            }
            catch (ObjectDisposedException)
            {
                return;
            }
            catch (SocketException ex) when (ex.SocketErrorCode == SocketError.ConnectionReset)
            {
                var nowTicks = Stopwatch.GetTimestamp();
                if (_lastControlLoopConnectionResetLogTicks == 0 ||
                    Stopwatch.GetElapsedTime(_lastControlLoopConnectionResetLogTicks, nowTicks) >= TimeSpan.FromSeconds(5))
                {
                    ReceiverTrace.Log($"Windows sender control loop ignored transient UDP reset from {_remoteHost}:{_remotePort}.");
                    _lastControlLoopConnectionResetLogTicks = nowTicks;
                }

                lock (_sync)
                {
                    _snapshot = _snapshot with
                    {
                        Status = $"Streaming {_remoteHost}:{_remotePort} (receiver control reconnecting)",
                    };
                }
                continue;
            }
            catch (Exception ex)
            {
                ReceiverTrace.Log(ex, "Windows sender control loop failed");
                lock (_sync)
                {
                    _snapshot = _snapshot with { Status = $"Control receive error: {ex.Message}" };
                }
                return;
            }

            if (!ProtocolParser.TryParse(result.Buffer, result.Buffer.Length, out var packet) || packet is null || packet.Type != TransportProtocol.TypeControl)
            {
                continue;
            }

            lock (_sync)
            {
            _snapshot = _snapshot with { ControlPacketsReceived = _snapshot.ControlPacketsReceived + 1 };
        }
            _lastControlPacketReceivedTicks = Stopwatch.GetTimestamp();

            if (ControlMessageParser.IsRequestKeyFrame(packet.Payload))
            {
                if (_lastCodecConfig is not null)
                {
                    SendPacket(_packetizer.BuildCodecConfigPacket(_lastCodecConfig), TransportProtocol.TypeCodecConfig);
                }

                lock (_sync)
                {
                    _snapshot = _snapshot with { LastControlKind = "request_keyframe" };
                }

                var nowTicks = Stopwatch.GetTimestamp();
                var sinceEncoderStartMs = _lastEncoderStartTicks <= 0
                    ? long.MaxValue
                    : (long)Stopwatch.GetElapsedTime(_lastEncoderStartTicks, nowTicks).TotalMilliseconds;
                var sinceLastRestartMs = _lastKeyFrameRestartTicks <= 0
                    ? long.MaxValue
                    : (long)Stopwatch.GetElapsedTime(_lastKeyFrameRestartTicks, nowTicks).TotalMilliseconds;

                if (sinceEncoderStartMs >= KeyFrameRequestStartupGraceMs &&
                    sinceLastRestartMs >= KeyFrameRequestCooldownMs)
                {
                    Interlocked.Exchange(ref _forceEncoderRestartRequested, 1);
                    _lastKeyFrameRestartTicks = nowTicks;
                }

                continue;
            }

            if (ControlMessageParser.IsReceiverStop(packet.Payload))
            {
                lock (_sync)
                {
                    _snapshot = _snapshot with { LastControlKind = "receiver_stop" };
                }
                ReceiverTrace.Log("Windows sender received receiver_stop control. Stopping sender immediately.");
                _ = Task.Run(() => Stop());
                return;
            }

            var latencyPulseRequest = LatencyPulseRequestControl.Parse(packet.Payload);
            if (latencyPulseRequest is not null)
            {
                var pendingPulse = new PendingLatencyPulse(
                    PulseId: Interlocked.Increment(ref _nextLatencyPulseId),
                    InputSeq: latencyPulseRequest.Seq,
                    Source: latencyPulseRequest.Source,
                    TriggerTicks: Stopwatch.GetTimestamp());

                lock (_sync)
                {
                    _pendingLatencyPulse = pendingPulse;
                    _snapshot = _snapshot with { LastControlKind = "latency_pulse_request" };
                }

                if (_latencyPulseFlashEnabled)
                {
                    _latencyPulseOverlay.Flash(_selectedMonitorBounds);
                }
                continue;
            }

            var feedback = ReceiverFeedbackControl.Parse(packet.Payload);
            if (feedback is not null)
            {
                lock (_sync)
                {
                    _snapshot = _snapshot with
                    {
                        LastControlKind = $"receiver_feedback:{feedback.Pressure}",
                        ReceiverPressure = feedback.Pressure,
                        ReceiverDecodeFps = feedback.DecodeFps,
                        ReceiverQueueDrops = feedback.QueueDrops,
                        ReceiverDecodeDeltaMs = feedback.DecodeDeltaMs,
                        ReceiverPresentDeltaMs = feedback.PresentDeltaMs,
                        PulseToAndroidEstimateMs = feedback.PulseEstimateMs,
                        InputToAndroidEstimateMs = feedback.InputEstimateMs,
                    };
                }
                _lastReceiverFeedbackTicks = Stopwatch.GetTimestamp();
                _lastControlPacketReceivedTicks = Stopwatch.GetTimestamp();
                ConsiderAdaptiveRelief(feedback);
                continue;
            }

            var remoteInput = RemoteInputControlMessage.Parse(packet.Payload);
            if (remoteInput is null || remoteInput.Seq <= Interlocked.Read(ref _lastAcceptedControlSeq))
            {
                continue;
            }

            var applied = remoteInput switch
            {
                RemoteGamepadStateMessage gamepadState => _gamepadInjector.TryApply(gamepadState),
                _ => _inputInjector.TryApply(remoteInput, _selectedMonitorBounds),
            };

            if (applied)
            {
                Interlocked.Exchange(ref _lastAcceptedControlSeq, remoteInput.Seq);
                _lastControlPacketReceivedTicks = Stopwatch.GetTimestamp();
                lock (_sync)
                {
                    _snapshot = _snapshot with
                    {
                        LastControlKind = remoteInput.GetType().Name,
                        GamepadStatus = _gamepadInjector.Status,
                        GamepadInput = _gamepadInjector.LastInputSummary,
                    };
                }
            }
        }
    }

    private async Task RelayRegistrationLoopAsync(CancellationToken cancellationToken)
    {
        using var timer = new PeriodicTimer(TimeSpan.FromSeconds(2));
        while (await timer.WaitForNextTickAsync(cancellationToken))
        {
            SendRelayRegistration();
            if (ShouldRefreshReceiverHandshake())
            {
                SendSessionConfig();
                var codecConfig = _lastCodecConfig;
                if (codecConfig is not null)
                {
                    SendPacket(_packetizer.BuildCodecConfigPacket(codecConfig), TransportProtocol.TypeCodecConfig);
                }
            }
        }
    }

    private bool ShouldRefreshReceiverHandshake()
    {
        var lastControlTicks = Interlocked.Read(ref _lastControlPacketReceivedTicks);
        if (lastControlTicks <= 0)
        {
            return true;
        }

        return Stopwatch.GetElapsedTime(lastControlTicks) >= TimeSpan.FromSeconds(3);
    }

    private void SendRelayRegistration()
    {
        var route = _relayRoute;
        if (route is null)
        {
            return;
        }

        SendPacket(
            ControlPacketBuilder.BuildRelayRegistration(
                sessionId: route.SessionId,
                sessionToken: route.SessionToken,
                role: "sender"),
            TransportProtocol.TypeControl);
    }

    private async Task CaptureLoopAsync(WindowsCaptureTargetInfo target, CancellationToken cancellationToken)
    {
        TryRaiseCurrentThreadPriority(ThreadPriority.Highest);
        MediaFoundationRuntime.Acquire();

        try
        {
            SenderCaptureContext? captureContext = null;
            GdiCaptureContext? gdiCaptureContext = null;
            var captureMode = "DXGI Desktop Duplication";
            int sourceWidth;
            int sourceHeight;

            try
            {
                using var factory = DXGI.CreateDXGIFactory1<IDXGIFactory1>();
                captureContext = CreateCaptureContext(factory, target);
                sourceWidth = captureContext.SourceWidth;
                sourceHeight = captureContext.SourceHeight;
            }
            catch (Exception ex) when (IsDesktopDuplicationUnsupported(ex))
            {
                ReceiverTrace.Log(ex, "Desktop Duplication unsupported; falling back to GDI capture");
                gdiCaptureContext = CreateGdiCaptureContext(target);
                sourceWidth = gdiCaptureContext.SourceWidth;
                sourceHeight = gdiCaptureContext.SourceHeight;
                captureMode = "GDI CopyFromScreen fallback";
            }

            using (captureContext)
            using (gdiCaptureContext)
            {
                while (!cancellationToken.IsCancellationRequested)
                {
                    try
                    {
                        switch (_encoderBackend)
                        {
                            case WindowsSenderEncoderBackend.NvidiaNvencNative:
                                try
                                {
                                    await RunResolvedEncoderBackendAsync(
                                        target,
                                        captureMode,
                                        captureContext,
                                        gdiCaptureContext,
                                        sourceWidth,
                                        sourceHeight,
                                        WindowsSenderEncoderBackend.NvidiaNvencNative,
                                        null,
                                        cancellationToken);
                                    return;
                                }
                                catch (Exception ex) when (ex is not SenderReconfigureRequestedException && !cancellationToken.IsCancellationRequested)
                                {
                                    ReceiverTrace.Log(ex, "Native NVENC failed; trying fallback encoder path");
                                    if (TryResolvePreferredEncoderBackend(nativeAllowed: false, out var nativeFallbackBackend, out var nativeFallbackFfmpegPath))
                                    {
                                        await RunResolvedEncoderBackendAsync(
                                            target,
                                            captureMode,
                                            captureContext,
                                            gdiCaptureContext,
                                            sourceWidth,
                                            sourceHeight,
                                            nativeFallbackBackend,
                                            nativeFallbackFfmpegPath,
                                            cancellationToken);
                                        return;
                                    }
                                }

                                using (var encoder = TryCreateEncoderOrFallback(sourceWidth, sourceHeight, target, cancellationToken))
                                {
                                    if (encoder is null)
                                    {
                                        return;
                                    }

                                    RunNativeEncoderLoop(target, captureMode, captureContext, gdiCaptureContext, encoder, WindowsSenderEncoderBackend.MediaFoundation.ToPathLabel(), cancellationToken);
                                }
                                return;

                            case WindowsSenderEncoderBackend.MediaFoundation:
                                await RunResolvedEncoderBackendAsync(
                                    target,
                                    captureMode,
                                    captureContext,
                                    gdiCaptureContext,
                                    sourceWidth,
                                    sourceHeight,
                                    WindowsSenderEncoderBackend.MediaFoundation,
                                    null,
                                    cancellationToken);
                                return;

                            case WindowsSenderEncoderBackend.NvidiaNvenc:
                            case WindowsSenderEncoderBackend.IntelQuickSync:
                                await RunResolvedEncoderBackendAsync(
                                    target,
                                    captureMode,
                                    captureContext,
                                    gdiCaptureContext,
                                    sourceWidth,
                                    sourceHeight,
                                    _encoderBackend,
                                    null,
                                    cancellationToken);
                                return;

                            case WindowsSenderEncoderBackend.FfmpegSoftware:
                                await RunResolvedEncoderBackendAsync(
                                    target,
                                    captureMode,
                                    captureContext,
                                    gdiCaptureContext,
                                    sourceWidth,
                                    sourceHeight,
                                    WindowsSenderEncoderBackend.FfmpegSoftware,
                                    null,
                                    cancellationToken);
                                return;

                            case WindowsSenderEncoderBackend.Auto:
                            default:
                                if (TryResolvePreferredEncoderBackend(captureContext is not null, out var autoBackend, out var autoFfmpegPath))
                                {
                                    await RunResolvedEncoderBackendAsync(
                                        target,
                                        captureMode,
                                        captureContext,
                                        gdiCaptureContext,
                                        sourceWidth,
                                        sourceHeight,
                                        autoBackend,
                                        autoFfmpegPath,
                                        cancellationToken);
                                    return;
                                }

                                using (var encoder = TryCreateEncoderOrFallback(sourceWidth, sourceHeight, target, cancellationToken))
                                {
                                    if (encoder is null)
                                    {
                                        return;
                                    }

                                    RunNativeEncoderLoop(target, captureMode, captureContext, gdiCaptureContext, encoder, WindowsSenderEncoderBackend.MediaFoundation.ToPathLabel(), cancellationToken);
                                }
                                return;
                        }
                    }
                    catch (SenderReconfigureRequestedException)
                    {
                        ApplyPendingAdaptiveStepIfAny(target.Bounds.Size);
                    }
                }
            }
        }
        catch (OperationCanceledException)
        {
        }
        catch (Exception ex)
        {
            ReceiverTrace.Log(ex, "Windows sender capture loop failed");
            lock (_sync)
            {
                _snapshot = _snapshot with
                {
                    Status = $"Sender error: {ex.Message}",
                    LastEncoderError = ex.Message,
                };
            }
        }
        finally
        {
            _inputInjector.ResetSession();
            MediaFoundationRuntime.Release();
        }
    }

    private async Task AudioLoopAsync(CancellationToken cancellationToken)
    {
        TryRaiseCurrentThreadPriority(ThreadPriority.AboveNormal);

        try
        {
            using var capture = new WasapiLoopbackCapture();
            var senderAudioConfig = BuildSenderAudioConfig(capture.WaveFormat);
            var startedAtTicks = Stopwatch.GetTimestamp();
            var completion = new TaskCompletionSource<object?>(TaskCreationOptions.RunContinuationsAsynchronously);

            capture.DataAvailable += (_, args) =>
            {
                try
                {
                    var payload = ConvertCapturedAudioToPcm16(capture.WaveFormat, args.Buffer, args.BytesRecorded, senderAudioConfig.Channels);
                    if (payload.Length == 0)
                    {
                        return;
                    }

                    var presentationTimeUs = Math.Max(
                        0L,
                        (long)Math.Round(Stopwatch.GetElapsedTime(startedAtTicks).TotalMilliseconds * 1_000.0));
                    var packets = _packetizer.PacketizeAudioFrame(_nextAudioFrameId++, presentationTimeUs, payload);
                    foreach (var packet in packets)
                    {
                        SendPacket(packet, TransportProtocol.TypeAudioFrame);
                    }
                }
                catch (Exception ex)
                {
                    ReceiverTrace.Log(ex, "Windows sender audio frame send failed");
                }
            };

            capture.RecordingStopped += (_, args) =>
            {
                if (args.Exception is not null && !cancellationToken.IsCancellationRequested)
                {
                    completion.TrySetException(args.Exception);
                    return;
                }

                completion.TrySetResult(null);
            };

            using var cancellationRegistration = cancellationToken.Register(() =>
            {
                try
                {
                    capture.StopRecording();
                }
                catch
                {
                }
            });

            SendAudioConfig(senderAudioConfig);
            UpdateAudioStatus($"PCM {senderAudioConfig.SampleRate} Hz / {senderAudioConfig.Channels} ch");
            capture.StartRecording();
            await completion.Task.ConfigureAwait(false);
        }
        catch (OperationCanceledException)
        {
        }
        catch (Exception ex)
        {
            ReceiverTrace.Log(ex, "Windows sender audio loopback failed");
            UpdateAudioStatus($"Unavailable: {ex.Message}");
        }
    }

    private void RunNativeEncoderLoop(
        WindowsCaptureTargetInfo target,
        string captureMode,
        SenderCaptureContext? captureContext,
        GdiCaptureContext? gdiCaptureContext,
        SenderEncoderContext encoder,
        string encoderPathLabel,
        CancellationToken cancellationToken)
    {
        SendSessionConfig();
        _lastEncoderStartTicks = Stopwatch.GetTimestamp();
        lock (_sync)
        {
            _snapshot = _snapshot with
            {
                Status = $"Streaming {target.UiLabel} -> {GetTransportTargetLabel()} ({captureMode})",
                EncoderPath = encoderPathLabel,
                LastEncoderError = "-",
            };
        }

        var nextFrameDueTicks = Stopwatch.GetTimestamp();

        while (!cancellationToken.IsCancellationRequested)
        {
            if (Interlocked.Exchange(ref _forceEncoderRestartRequested, 0) != 0)
            {
                ApplyPendingAdaptiveStepIfAny(_selectedMonitorBounds.Size);
                encoder.Reinitialize();
                SendSessionConfig();
            }

            if (captureContext is not null)
            {
                IDXGIResource? frameResource = null;
                var frameAcquired = false;
                try
                {
                    var acquireStartTicks = Stopwatch.GetTimestamp();
                    var acquireResult = captureContext.Duplication.AcquireNextFrame(100, out var frameInfo, out frameResource);
                    if (acquireResult == Vortice.DXGI.ResultCode.WaitTimeout)
                    {
                        IncrementNativeDxgiTimeout();
                        continue;
                    }

                    acquireResult.CheckError();
                    frameAcquired = true;
                    using var texture = frameResource!.QueryInterface<ID3D11Texture2D>();
                    captureContext.Context.CopyResource(captureContext.StagingTexture, texture);

                    var nowTicks = Stopwatch.GetTimestamp();
                    UpdateCaptureStats(nowTicks, frameInfo.AccumulatedFrames > 1 ? frameInfo.AccumulatedFrames - 1 : 0);

                    if (nowTicks < nextFrameDueTicks)
                    {
                        IncrementDroppedFrames();
                        continue;
                    }

                    nextFrameDueTicks = nowTicks + (long)(_sampleDurationHns / 10_000_000.0 * Stopwatch.Frequency);
                    var frameBytes = CopyStagingTextureToManagedBuffer(captureContext);
                    encoder.WriteFrame(frameBytes, captureContext.SourceWidth * 4, _sampleTimeHns);
                    _sampleTimeHns += _sampleDurationHns;
                }
                finally
                {
                    frameResource?.Dispose();
                    if (frameAcquired)
                    {
                        captureContext.Duplication.ReleaseFrame().CheckError();
                    }
                }

                continue;
            }

            if (gdiCaptureContext is null)
            {
                throw new InvalidOperationException("No capture path available");
            }

            var gdiNowTicks = Stopwatch.GetTimestamp();
            UpdateCaptureStats(gdiNowTicks, 0);
            if (gdiNowTicks < nextFrameDueTicks)
            {
                IncrementDroppedFrames();
                continue;
            }

            nextFrameDueTicks = gdiNowTicks + (long)(_sampleDurationHns / 10_000_000.0 * Stopwatch.Frequency);
            var gdiFrameBytes = CaptureScreenBytesGdi(gdiCaptureContext);
            encoder.WriteFrame(gdiFrameBytes, gdiCaptureContext.SourceWidth * 4, _sampleTimeHns);
            _sampleTimeHns += _sampleDurationHns;
        }
    }

    private void RunNativeNvencLoop(
        WindowsCaptureTargetInfo target,
        string captureMode,
        SenderCaptureContext captureContext,
        CancellationToken cancellationToken)
    {
        NvEncNativeBridge? encoder = null;
        using var latestFrameTexture = captureContext.Device.CreateTexture2D(
            new Texture2DDescription(
                Format.B8G8R8A8_UNorm,
                (uint)captureContext.SourceWidth,
                (uint)captureContext.SourceHeight,
                1,
                1,
                BindFlags.None,
                ResourceUsage.Default,
                CpuAccessFlags.None,
                1,
                0,
                ResourceOptionFlags.None));
        var hasLatestFrame = false;
        var lastAcquireWaitMs = 0.0;
        var lastAcquireProcessMs = 0.0;

        try
        {
            encoder = CreateNativeNvencBridge(captureContext.Device);
            ReceiverTrace.Log(
                $"Native NVENC sender started: codec={_selectedCodec.ToUiLabel()}; " +
                $"path={WindowsSenderEncoderBackend.NvidiaNvencNative.ToPathLabel()} / DXGI Desktop Duplication; " +
                $"target={_targetWidth}x{_targetHeight}@{_targetFps}; bitrate={_targetBitrateBps}");
            SendSessionConfig();
            _lastEncoderStartTicks = Stopwatch.GetTimestamp();
            lock (_sync)
            {
                _snapshot = _snapshot with
                {
                    Status = $"Streaming {target.UiLabel} -> {GetTransportTargetLabel()} ({captureMode})",
                    EncoderPath = $"{WindowsSenderEncoderBackend.NvidiaNvencNative.ToPathLabel()} / DXGI Desktop Duplication",
                    LastEncoderError = "-",
                };
            }

            var nextFrameDueTicks = Stopwatch.GetTimestamp();
            var frameIntervalTicks = Math.Max(1L, (long)(_sampleDurationHns / 10_000_000.0 * Stopwatch.Frequency));

            while (!cancellationToken.IsCancellationRequested)
            {
                var forceIdr = false;
                if (Interlocked.Exchange(ref _forceEncoderRestartRequested, 0) != 0)
                {
                    var previousWidth = _targetWidth;
                    var previousHeight = _targetHeight;
                    var previousFps = _targetFps;
                    var previousBitrate = _targetBitrateBps;
                    var previousGop = ComputeCurrentGopLength();

                    ApplyPendingAdaptiveStepIfAny(_selectedMonitorBounds.Size);

                    if (_targetWidth != previousWidth || _targetHeight != previousHeight)
                    {
                        encoder.Dispose();
                        encoder = CreateNativeNvencBridge(captureContext.Device);
                    }
                    else if (_targetBitrateBps != previousBitrate || _targetFps != previousFps || ComputeCurrentGopLength() != previousGop)
                    {
                        encoder.Reconfigure(_targetBitrateBps, _targetFps, ComputeCurrentGopLength());
                    }

                    forceIdr = true;
                    SendSessionConfig();
                }

                var loopStartTicks = Stopwatch.GetTimestamp();
                IDXGIResource? frameResource = null;
                var frameAcquired = false;
                try
                {
                    var acquireStartTicks = Stopwatch.GetTimestamp();
                    var acquireResult = captureContext.Duplication.AcquireNextFrame(0, out var frameInfo, out frameResource);
                    var acquireReturnTicks = Stopwatch.GetTimestamp();
                    if (acquireResult == Vortice.DXGI.ResultCode.WaitTimeout)
                    {
                        IncrementNativeDxgiTimeout();
                        lastAcquireWaitMs = Stopwatch.GetElapsedTime(acquireStartTicks, acquireReturnTicks).TotalMilliseconds;
                        lastAcquireProcessMs = 0;
                    }
                    else
                    {
                        acquireResult.CheckError();
                        frameAcquired = true;
                        using var texture = frameResource!.QueryInterface<ID3D11Texture2D>();

                        var nowTicks = Stopwatch.GetTimestamp();
                        captureContext.Context.CopyResource(latestFrameTexture, texture);
                        var acquireProcessDoneTicks = Stopwatch.GetTimestamp();
                        lastAcquireWaitMs = Stopwatch.GetElapsedTime(acquireStartTicks, acquireReturnTicks).TotalMilliseconds;
                        lastAcquireProcessMs = Stopwatch.GetElapsedTime(acquireReturnTicks, acquireProcessDoneTicks).TotalMilliseconds;
                        hasLatestFrame = true;
                        UpdateCaptureStats(nowTicks, frameInfo.AccumulatedFrames > 1 ? frameInfo.AccumulatedFrames - 1 : 0);

                        if (nowTicks < nextFrameDueTicks)
                        {
                            IncrementNativePacedSkip();
                        }
                    }

                    var submitTicks = Stopwatch.GetTimestamp();
                    if (!hasLatestFrame || submitTicks < nextFrameDueTicks)
                    {
                        Thread.Sleep(0);
                        continue;
                    }

                    var encodeStartTicks = Stopwatch.GetTimestamp();
                    UpdateSubmitStats(encodeStartTicks);
                    encoder.EncodeFrame(latestFrameTexture.NativePointer, _sampleTimeHns, forceIdr);
                    var encodeDoneTicks = Stopwatch.GetTimestamp();
                    var packets = encoder.DrainPackets();
                    foreach (var packet in packets)
                    {
                        ProcessEncodedPayload(packet.Payload, Math.Max(0, packet.TimestampHns / 10), annexBInput: true);
                    }
                    var packetizeDoneTicks = Stopwatch.GetTimestamp();

                    if (packets.Count > 0)
                    {
                        UpdateEncodeStats(packetizeDoneTicks);
                    }

                    UpdateNativeStageStats(
                        acquireWaitMs: lastAcquireWaitMs,
                        acquireProcessMs: lastAcquireProcessMs,
                        encodeCallMs: Stopwatch.GetElapsedTime(encodeStartTicks, encodeDoneTicks).TotalMilliseconds,
                        drainPacketizeMs: Stopwatch.GetElapsedTime(encodeDoneTicks, packetizeDoneTicks).TotalMilliseconds);

                    do
                    {
                        nextFrameDueTicks += frameIntervalTicks;
                    }
                    while (nextFrameDueTicks <= packetizeDoneTicks);
                    _sampleTimeHns += _sampleDurationHns;
                }
                finally
                {
                    frameResource?.Dispose();
                    if (frameAcquired)
                    {
                        captureContext.Duplication.ReleaseFrame().CheckError();
                    }
                }

                if (!hasLatestFrame)
                {
                    Thread.Sleep(1);
                    continue;
                }

                var afterLoopTicks = Stopwatch.GetTimestamp();
                if (afterLoopTicks < nextFrameDueTicks)
                {
                    Thread.Sleep(0);
                }
            }
        }
        finally
        {
            encoder?.Dispose();
        }
    }

    private async Task RunFfmpegHardwareCaptureLoopAsync(
        WindowsCaptureTargetInfo target,
        string ffmpegPath,
        WindowsSenderEncoderBackend encoderBackend,
        CancellationToken cancellationToken)
    {
        var preferredBackend = TryResolveDxgiOutputIndex(target.DeviceName, out _) && _targetFps >= 50
            ? FfmpegCaptureInputBackend.DdaGrab
            : FfmpegCaptureInputBackend.GdiGrab;
        var fallbackBackend = preferredBackend == FfmpegCaptureInputBackend.DdaGrab
            ? FfmpegCaptureInputBackend.GdiGrab
            : (FfmpegCaptureInputBackend?)null;

        try
        {
            await RunFfmpegHardwareCaptureLoopWithBackendAsync(
                target,
                ffmpegPath,
                encoderBackend,
                preferredBackend,
                cancellationToken).ConfigureAwait(false);
        }
        catch (Exception ex) when (ex is not SenderReconfigureRequestedException && fallbackBackend is not null && !cancellationToken.IsCancellationRequested)
        {
            ReceiverTrace.Log(ex, $"FFmpeg {encoderBackend.ToUiLabel()} capture failed on {preferredBackend}; retrying with {fallbackBackend.Value}");
            await RunFfmpegHardwareCaptureLoopWithBackendAsync(
                target,
                ffmpegPath,
                encoderBackend,
                fallbackBackend.Value,
                cancellationToken).ConfigureAwait(false);
        }
    }

    private async Task RunResolvedEncoderBackendAsync(
        WindowsCaptureTargetInfo target,
        string captureMode,
        SenderCaptureContext? captureContext,
        GdiCaptureContext? gdiCaptureContext,
        int sourceWidth,
        int sourceHeight,
        WindowsSenderEncoderBackend backend,
        string? ffmpegPath,
        CancellationToken cancellationToken)
    {
        ReceiverTrace.Log(
            $"Windows sender resolved backend: {backend}; codec={_selectedCodec.ToUiLabel()}; " +
            $"captureMode={captureMode}; target={_targetWidth}x{_targetHeight}@{_targetFps}; bitrate={_targetBitrateBps}; " +
            $"dxgi={(captureContext is not null)}; ffmpegPath={(string.IsNullOrWhiteSpace(ffmpegPath) ? "-" : ffmpegPath)}");

        switch (backend)
        {
            case WindowsSenderEncoderBackend.NvidiaNvencNative:
                if (captureContext is null)
                {
                    throw new InvalidOperationException("Native NVENC requires DXGI Desktop Duplication capture.");
                }

                RunNativeNvencLoop(target, captureMode, captureContext, cancellationToken);
                return;

            case WindowsSenderEncoderBackend.MediaFoundation:
                using (var encoder = CreateEncoder(sourceWidth, sourceHeight))
                {
                    RunNativeEncoderLoop(target, captureMode, captureContext, gdiCaptureContext, encoder, WindowsSenderEncoderBackend.MediaFoundation.ToPathLabel(), cancellationToken);
                }
                return;

            case WindowsSenderEncoderBackend.NvidiaNvenc:
            case WindowsSenderEncoderBackend.IntelQuickSync:
                if (string.IsNullOrWhiteSpace(ffmpegPath) && !TryResolveFfmpegExecutable(out ffmpegPath))
                {
                    throw new InvalidOperationException("ffmpeg.exe was not found for the selected hardware encoder backend.");
                }

                await RunFfmpegHardwareCaptureLoopAsync(target, ffmpegPath!, backend, cancellationToken);
                return;

            case WindowsSenderEncoderBackend.FfmpegSoftware:
                if (string.IsNullOrWhiteSpace(ffmpegPath) && !TryResolveFfmpegExecutable(out ffmpegPath))
                {
                    throw new InvalidOperationException("ffmpeg.exe was not found for software sender mode.");
                }

                await RunFfmpegSoftwareCaptureLoopAsync(target, ffmpegPath!, cancellationToken);
                return;

            default:
                throw new ArgumentOutOfRangeException(nameof(backend), backend, null);
        }
    }

    private async Task RunFfmpegHardwareCaptureLoopWithBackendAsync(
        WindowsCaptureTargetInfo target,
        string ffmpegPath,
        WindowsSenderEncoderBackend encoderBackend,
        FfmpegCaptureInputBackend captureBackend,
        CancellationToken cancellationToken)
    {
        _lastCodecConfig = null;
        _nextFrameId = 1;

        using var process = new Process
        {
            StartInfo = BuildFfmpegHardwareCaptureStartInfo(ffmpegPath, target, encoderBackend, captureBackend),
        };

        if (!process.Start())
        {
            throw new InvalidOperationException($"Failed to start ffmpeg {encoderBackend.ToUiLabel()} encoder.");
        }

        TryRaiseProcessPriority(process, ProcessPriorityClass.High);
        using var cancellationRegistration = cancellationToken.Register(() => TryTerminateProcess(process));
        var stderrTask = process.StandardError.ReadToEndAsync();
        var reader = CreateEncodedAccessUnitReader();
        var startedAtTicks = Stopwatch.GetTimestamp();
        long lastPresentationTimeUs = 0;
        var buffer = ArrayPool<byte>.Shared.Rent(64 * 1024);

        SendSessionConfig();
        lock (_sync)
        {
            _lastEncoderStartTicks = Stopwatch.GetTimestamp();
            _snapshot = _snapshot with
            {
                Status = $"Streaming {target.UiLabel} -> {GetTransportTargetLabel()} (FFmpeg {encoderBackend.ToUiLabel()} / {FormatCaptureBackendLabel(captureBackend)})",
                EncoderPath = $"{encoderBackend.ToPathLabel()} / {FormatCaptureBackendLabel(captureBackend)}",
                LastEncoderError = "-",
            };
        }

        try
        {
            while (!cancellationToken.IsCancellationRequested)
            {
                if (Interlocked.Exchange(ref _forceEncoderRestartRequested, 0) != 0)
                {
                    throw new SenderReconfigureRequestedException();
                }

                var bytesRead = await process.StandardOutput.BaseStream.ReadAsync(buffer.AsMemory(0, buffer.Length), cancellationToken);
                if (bytesRead <= 0)
                {
                    break;
                }

                reader.Feed(buffer.AsSpan(0, bytesRead), (bytes, _) =>
                {
                    var nowTicks = Stopwatch.GetTimestamp();
                    var presentationTimeUs = Math.Max(
                        lastPresentationTimeUs + 1,
                        (long)Math.Round(Stopwatch.GetElapsedTime(startedAtTicks).TotalMilliseconds * 1_000.0));
                    lastPresentationTimeUs = presentationTimeUs;
                    ProcessEncodedPayload(bytes, presentationTimeUs, annexBInput: true);
                    UpdateCaptureStats(nowTicks, 0);
                    UpdateEncodeStats(nowTicks);
                });
            }

            reader.Complete((bytes, _) =>
            {
                var nowTicks = Stopwatch.GetTimestamp();
                var presentationTimeUs = Math.Max(
                    lastPresentationTimeUs + 1,
                    (long)Math.Round(Stopwatch.GetElapsedTime(startedAtTicks).TotalMilliseconds * 1_000.0));
                lastPresentationTimeUs = presentationTimeUs;
                ProcessEncodedPayload(bytes, presentationTimeUs, annexBInput: true);
                UpdateCaptureStats(nowTicks, 0);
                UpdateEncodeStats(nowTicks);
            });

            if (cancellationToken.IsCancellationRequested)
            {
                return;
            }

            var stderr = await stderrTask.ConfigureAwait(false);
            if (!process.WaitForExit(500))
            {
                throw new IOException($"ffmpeg {encoderBackend.ToUiLabel()} encoder stopped responding");
            }

            if (process.ExitCode != 0)
            {
                var errorMessage = string.IsNullOrWhiteSpace(stderr)
                    ? $"ffmpeg {encoderBackend.ToUiLabel()} exited with code {process.ExitCode}"
                    : stderr.Trim();
                throw new IOException(errorMessage);
            }
        }
        finally
        {
            ArrayPool<byte>.Shared.Return(buffer);
            TryTerminateProcess(process);
        }
    }

    private async Task PumpFfmpegOutputAsync(Process process, CancellationToken cancellationToken)
    {
        var reader = CreateEncodedAccessUnitReader();
        var startedAtTicks = Stopwatch.GetTimestamp();
        long lastPresentationTimeUs = 0;
        var buffer = ArrayPool<byte>.Shared.Rent(64 * 1024);

        try
        {
            while (!cancellationToken.IsCancellationRequested)
            {
                if (Interlocked.Exchange(ref _forceEncoderRestartRequested, 0) != 0)
                {
                    throw new SenderReconfigureRequestedException();
                }

                var bytesRead = await process.StandardOutput.BaseStream.ReadAsync(buffer.AsMemory(0, buffer.Length), cancellationToken);
                if (bytesRead <= 0)
                {
                    break;
                }

                reader.Feed(buffer.AsSpan(0, bytesRead), (bytes, _) =>
                {
                    var nowTicks = Stopwatch.GetTimestamp();
                    var presentationTimeUs = Math.Max(
                        lastPresentationTimeUs + 1,
                        (long)Math.Round(Stopwatch.GetElapsedTime(startedAtTicks).TotalMilliseconds * 1_000.0));
                    lastPresentationTimeUs = presentationTimeUs;
                    ProcessEncodedPayload(bytes, presentationTimeUs, annexBInput: true);
                    UpdateEncodeStats(nowTicks);
                });
            }

            reader.Complete((bytes, _) =>
            {
                var nowTicks = Stopwatch.GetTimestamp();
                var presentationTimeUs = Math.Max(
                    lastPresentationTimeUs + 1,
                    (long)Math.Round(Stopwatch.GetElapsedTime(startedAtTicks).TotalMilliseconds * 1_000.0));
                lastPresentationTimeUs = presentationTimeUs;
                ProcessEncodedPayload(bytes, presentationTimeUs, annexBInput: true);
                UpdateEncodeStats(nowTicks);
            });
        }
        finally
        {
            ArrayPool<byte>.Shared.Return(buffer);
        }
    }

    private SenderEncoderContext? TryCreateEncoderOrFallback(int sourceWidth, int sourceHeight, WindowsCaptureTargetInfo target, CancellationToken cancellationToken)
    {
        try
        {
            return CreateEncoder(sourceWidth, sourceHeight);
        }
        catch (Exception ex)
        {
            if (!TryResolveFfmpegExecutable(out var ffmpegPath))
            {
                throw new InvalidOperationException(
                    $"{ex.Message}{Environment.NewLine}Media Foundation encoder unavailable and ffmpeg.exe was not found for software fallback.",
                    ex);
            }

            ReceiverTrace.Log(ex, "Media Foundation encoder unavailable; falling back to ffmpeg software sender");
            RunFfmpegSoftwareCaptureLoopAsync(target, ffmpegPath, cancellationToken).GetAwaiter().GetResult();
            return null;
        }
    }

    private NvEncNativeBridge CreateNativeNvencBridge(ID3D11Device device)
    {
        return new NvEncNativeBridge(
            device.NativePointer,
            width: _targetWidth,
            height: _targetHeight,
            codec: _selectedCodec,
            bitrateBps: _targetBitrateBps,
            fps: _targetFps,
            gopLength: ComputeCurrentGopLength(),
            gamePreset: IsGamePreset());
    }

    private bool TryResolvePreferredEncoderBackend(bool nativeAllowed, out WindowsSenderEncoderBackend backend, out string ffmpegPath)
    {
        backend = WindowsSenderEncoderBackend.Auto;
        ffmpegPath = string.Empty;
        UpdateAutoEncoderSelection("-");
        var capabilities = GetSenderCapabilityProbe();

        if (_selectedCodec.IsAv1())
        {
            if (capabilities.NvencAv1 && capabilities.FfmpegAvailable)
            {
                TryResolveFfmpegExecutable(out ffmpegPath);
                backend = WindowsSenderEncoderBackend.NvidiaNvenc;
                UpdateAutoEncoderSelection("NVIDIA NVENC");
                ReceiverTrace.Log($"Windows sender auto encoder selected: {backend} for {_selectedCodec.ToUiLabel()}");
                return true;
            }

            if (capabilities.QuickSyncAv1 && capabilities.FfmpegAvailable)
            {
                TryResolveFfmpegExecutable(out ffmpegPath);
                backend = WindowsSenderEncoderBackend.IntelQuickSync;
                UpdateAutoEncoderSelection("Intel Quick Sync");
                ReceiverTrace.Log($"Windows sender auto encoder selected: {backend} for {_selectedCodec.ToUiLabel()}");
                return true;
            }

            if (capabilities.FfmpegAvailable && capabilities.SoftwareAv1)
            {
                TryResolveFfmpegExecutable(out ffmpegPath);
                backend = WindowsSenderEncoderBackend.FfmpegSoftware;
                UpdateAutoEncoderSelection("Software");
                ReceiverTrace.Log($"Windows sender auto encoder selected: {backend} for {_selectedCodec.ToUiLabel()}");
                return true;
            }

            UpdateAutoEncoderSelection("Software");
            return false;
        }

        if (nativeAllowed && capabilities.HasNvidiaAdapter && capabilities.SupportsNativeNvenc(_selectedCodec))
        {
            backend = WindowsSenderEncoderBackend.NvidiaNvencNative;
            UpdateAutoEncoderSelection("NVIDIA NVENC Native");
            ReceiverTrace.Log($"Windows sender auto encoder selected: {backend} for {_selectedCodec.ToUiLabel()}");
            return true;
        }

        if (capabilities.HasNvidiaAdapter &&
            capabilities.FfmpegAvailable &&
            ((_selectedCodec.IsHevc() && capabilities.NvencHevc) || (!_selectedCodec.IsHevc() && capabilities.NvencAvc)))
        {
            TryResolveFfmpegExecutable(out ffmpegPath);
            backend = WindowsSenderEncoderBackend.NvidiaNvenc;
            UpdateAutoEncoderSelection("NVIDIA NVENC");
            ReceiverTrace.Log($"Windows sender auto encoder selected: {backend} for {_selectedCodec.ToUiLabel()}");
            return true;
        }

        if (capabilities.HasIntelAdapter &&
            capabilities.FfmpegAvailable &&
            ((_selectedCodec.IsHevc() && capabilities.QuickSyncHevc) || (!_selectedCodec.IsHevc() && capabilities.QuickSyncAvc)))
        {
            TryResolveFfmpegExecutable(out ffmpegPath);
            backend = WindowsSenderEncoderBackend.IntelQuickSync;
            UpdateAutoEncoderSelection("Intel Quick Sync");
            ReceiverTrace.Log($"Windows sender auto encoder selected: {backend} for {_selectedCodec.ToUiLabel()}");
            return true;
        }

        if ((_selectedCodec.IsHevc() && capabilities.MediaFoundationHevc) ||
            (!_selectedCodec.IsHevc() && !_selectedCodec.IsAv1() && capabilities.MediaFoundationAvc))
        {
            UpdateAutoEncoderSelection("Media Foundation");
            backend = WindowsSenderEncoderBackend.MediaFoundation;
            ReceiverTrace.Log($"Windows sender auto encoder selected: {backend} for {_selectedCodec.ToUiLabel()}");
            return true;
        }

        if (capabilities.FfmpegAvailable &&
            ((_selectedCodec.IsHevc() && capabilities.SoftwareHevc) ||
             (!_selectedCodec.IsHevc() && !_selectedCodec.IsAv1() && capabilities.SoftwareAvc)))
        {
            TryResolveFfmpegExecutable(out ffmpegPath);
            backend = WindowsSenderEncoderBackend.FfmpegSoftware;
            UpdateAutoEncoderSelection("Software");
            ReceiverTrace.Log($"Windows sender auto encoder selected: {backend} for {_selectedCodec.ToUiLabel()}");
            return true;
        }

        UpdateAutoEncoderSelection("Software");
        return false;
    }

    private void UpdateAutoEncoderSelection(string selected)
    {
        lock (_sync)
        {
            _snapshot = _snapshot with
            {
                AutoEncoderSelected = string.IsNullOrWhiteSpace(selected) ? "-" : selected,
            };
        }
    }

    private int ComputeCurrentGopLength() =>
        Math.Max(1, _targetFps * Math.Max(1, _senderSpec.KeyFrameIntervalSeconds));

    internal static SenderCapabilityProbeResult GetSenderCapabilityProbe()
    {
        lock (s_capabilityProbeSync)
        {
            if (s_capabilityProbe is not null)
            {
                return s_capabilityProbe;
            }

            var ffmpegAvailable = TryResolveFfmpegExecutable(out var ffmpegPath);
            var (hasNvidiaAdapter, hasIntelAdapter) = ProbeGraphicsAdapters();
            var result = new SenderCapabilityProbeResult(
                FfmpegAvailable: ffmpegAvailable,
                HasNvidiaAdapter: hasNvidiaAdapter,
                HasIntelAdapter: hasIntelAdapter,
                NativeNvencAvc: hasNvidiaAdapter && ProbeNativeNvencEncoder(WindowsVideoCodec.H264Avc),
                NativeNvencHevc: hasNvidiaAdapter && ProbeNativeNvencEncoder(WindowsVideoCodec.H265Hevc),
                NvencAvc: ffmpegAvailable && hasNvidiaAdapter && ProbeFfmpegEncoder(ffmpegPath, "h264_nvenc"),
                NvencHevc: ffmpegAvailable && hasNvidiaAdapter && ProbeFfmpegEncoder(ffmpegPath, "hevc_nvenc"),
                NvencAv1: ffmpegAvailable && hasNvidiaAdapter && ProbeFfmpegEncoder(ffmpegPath, "av1_nvenc"),
                QuickSyncAvc: ffmpegAvailable && hasIntelAdapter && ProbeFfmpegEncoder(ffmpegPath, "h264_qsv"),
                QuickSyncHevc: ffmpegAvailable && hasIntelAdapter && ProbeFfmpegEncoder(ffmpegPath, "hevc_qsv"),
                QuickSyncAv1: ffmpegAvailable && hasIntelAdapter && ProbeFfmpegEncoder(ffmpegPath, "av1_qsv"),
                SoftwareAvc: ffmpegAvailable && ProbeFfmpegEncoder(ffmpegPath, "libx264"),
                SoftwareHevc: ffmpegAvailable && ProbeFfmpegEncoder(ffmpegPath, "libx265"),
                SoftwareAv1: ffmpegAvailable && ProbeFfmpegEncoder(ffmpegPath, "libsvtav1"),
                MediaFoundationAvc: ProbeMediaFoundationEncoder(WindowsVideoCodec.H264Avc),
                MediaFoundationHevc: ProbeMediaFoundationEncoder(WindowsVideoCodec.H265Hevc));
            s_capabilityProbe = result;
            return result;
        }
    }

    private static bool ProbeNativeNvencEncoder(WindowsVideoCodec codec)
    {
        try
        {
            return string.IsNullOrWhiteSpace(NvEncNativeBridge.TryProbe(codec));
        }
        catch (Exception ex)
        {
            ReceiverTrace.Log(ex, $"Windows sender native NVENC probe failed: {codec.ToUiLabel()}");
            return false;
        }
    }

    private static (bool HasNvidiaAdapter, bool HasIntelAdapter) ProbeGraphicsAdapters()
    {
        try
        {
            using var factory = DXGI.CreateDXGIFactory1<IDXGIFactory1>();
            var hasNvidia = false;
            var hasIntel = false;
            for (uint adapterIndex = 0; ; adapterIndex++)
            {
                var adapterResult = factory.EnumAdapters1(adapterIndex, out var adapter);
                if (adapterResult.Failure || adapter is null)
                {
                    break;
                }

                using (adapter)
                {
                    var desc = adapter.Description1;
                    const int NvidiaVendorId = 0x10DE;
                    const int IntelVendorId = 0x8086;
                    if (desc.VendorId == NvidiaVendorId)
                    {
                        hasNvidia = true;
                    }
                    else if (desc.VendorId == IntelVendorId)
                    {
                        hasIntel = true;
                    }
                }
            }

            return (hasNvidia, hasIntel);
        }
        catch (Exception ex)
        {
            ReceiverTrace.Log(ex, "Windows sender graphics adapter probe failed");
            return (false, false);
        }
    }

    private static bool ProbeFfmpegEncoder(string ffmpegPath, string encoderName)
    {
        try
        {
            var startInfo = new ProcessStartInfo
            {
                FileName = ffmpegPath,
                UseShellExecute = false,
                RedirectStandardError = true,
                RedirectStandardOutput = true,
                CreateNoWindow = true,
            };
            startInfo.ArgumentList.Add("-hide_banner");
            startInfo.ArgumentList.Add("-loglevel");
            startInfo.ArgumentList.Add("error");
            startInfo.ArgumentList.Add("-f");
            startInfo.ArgumentList.Add("lavfi");
            startInfo.ArgumentList.Add("-i");
            startInfo.ArgumentList.Add("color=c=black:s=16x16:r=1");
            startInfo.ArgumentList.Add("-frames:v");
            startInfo.ArgumentList.Add("1");
            startInfo.ArgumentList.Add("-an");
            startInfo.ArgumentList.Add("-sn");
            startInfo.ArgumentList.Add("-c:v");
            startInfo.ArgumentList.Add(encoderName);
            startInfo.ArgumentList.Add("-f");
            startInfo.ArgumentList.Add("null");
            startInfo.ArgumentList.Add("-");

            using var process = Process.Start(startInfo);
            if (process is null)
            {
                return false;
            }

            process.WaitForExit(5000);
            return process.ExitCode == 0;
        }
        catch (Exception ex)
        {
            ReceiverTrace.Log(ex, $"Windows sender ffmpeg encoder probe failed: {encoderName}");
            return false;
        }
    }

    private static bool ProbeMediaFoundationEncoder(WindowsVideoCodec codec)
    {
        if (codec.IsAv1())
        {
            return false;
        }

        try
        {
            MediaFoundationRuntime.Acquire();
            try
            {
                var outputType = new RegisterTypeInfo
                {
                    GuidMajorType = MediaTypeGuids.Video,
                    GuidSubtype = codec.ToMediaFoundationSubtype(),
                };
                var activates = MediaFactory.MFTEnumEx(
                    TransformCategoryGuids.VideoEncoder,
                    (uint)(EnumFlag.EnumFlagHardware | EnumFlag.EnumFlagSortandfilter),
                    null,
                    outputType);
                foreach (var activate in activates)
                {
                    activate.Dispose();
                    return true;
                }

                return false;
            }
            finally
            {
                MediaFoundationRuntime.Release();
            }
        }
        catch (Exception ex)
        {
            ReceiverTrace.Log(ex, $"Windows sender Media Foundation encoder probe failed: {codec.ToUiLabel()}");
            return false;
        }
    }

    private SenderCaptureContext CreateCaptureContext(IDXGIFactory1 factory, WindowsCaptureTargetInfo target)
    {
        for (uint adapterIndex = 0; ; adapterIndex++)
        {
            IDXGIAdapter1? adapter = null;
            var adapterResult = factory.EnumAdapters1(adapterIndex, out adapter);
            if (adapterResult.Failure)
            {
                break;
            }

            try
            {
                for (uint outputIndex = 0; ; outputIndex++)
                {
                    IDXGIOutput? output = null;
                    var outputResult = adapter!.EnumOutputs(outputIndex, out output);
                    if (outputResult.Failure)
                    {
                        break;
                    }

                    try
                    {
                        var description = output!.Description;
                        if (!string.Equals(description.DeviceName, target.DeviceName, StringComparison.OrdinalIgnoreCase))
                        {
                            continue;
                        }

                        using var output1 = output.QueryInterface<IDXGIOutput1>();
                        D3D11.D3D11CreateDevice(
                            adapter,
                            DriverType.Unknown,
                            DeviceCreationFlags.BgraSupport | DeviceCreationFlags.VideoSupport,
                            PreferredFeatureLevels,
                            out ID3D11Device device,
                            out ID3D11DeviceContext context).CheckError();
                        using (var multithread = context.QueryInterfaceOrNull<ID3D11Multithread>())
                        {
                            multithread?.SetMultithreadProtected(true);
                        }

                        var duplication = output1.DuplicateOutput(device);
                        var mode = duplication.Description.ModeDescription;
                        var stagingTexture = device.CreateTexture2D(
                            new Texture2DDescription(
                                mode.Format,
                                mode.Width,
                                mode.Height,
                                1,
                                1,
                                BindFlags.None,
                                ResourceUsage.Staging,
                                CpuAccessFlags.Read,
                                1,
                                0,
                                ResourceOptionFlags.None));

                        return new SenderCaptureContext(
                            Adapter: adapter,
                            Device: device,
                            Context: context,
                            Duplication: duplication,
                            StagingTexture: stagingTexture,
                            SourceWidth: (int)mode.Width,
                            SourceHeight: (int)mode.Height);
                    }
                    finally
                    {
                        output?.Dispose();
                    }
                }
            }
            catch
            {
                adapter?.Dispose();
                throw;
            }
        }

        throw new InvalidOperationException($"Desktop Duplication output not found for {target.DeviceName}");
    }

    private static GdiCaptureContext CreateGdiCaptureContext(WindowsCaptureTargetInfo target)
    {
        var bitmap = new Bitmap(target.Bounds.Width, target.Bounds.Height, PixelFormat.Format32bppArgb);
        var graphics = Graphics.FromImage(bitmap);
        return new GdiCaptureContext(target.Bounds, bitmap, graphics);
    }

    private byte[] CopyStagingTextureToManagedBuffer(SenderCaptureContext captureContext)
    {
        var mapped = captureContext.Context.Map(captureContext.StagingTexture, 0, MapMode.Read, Vortice.Direct3D11.MapFlags.None);
        try
        {
            var totalBytes = captureContext.SourceWidth * captureContext.SourceHeight * 4;
            var managed = new byte[totalBytes];
            var rowBytes = captureContext.SourceWidth * 4;
            for (var y = 0; y < captureContext.SourceHeight; y++)
            {
                var sourceRow = IntPtr.Add(mapped.DataPointer, checked((int)(y * mapped.RowPitch)));
                Marshal.Copy(sourceRow, managed, y * rowBytes, rowBytes);
            }

            return managed;
        }
        finally
        {
            captureContext.Context.Unmap(captureContext.StagingTexture, 0);
        }
    }

    private static byte[] CaptureScreenBytesGdi(GdiCaptureContext captureContext)
    {
        captureContext.Graphics.CopyFromScreen(
            captureContext.Bounds.Location,
            Point.Empty,
            captureContext.Bounds.Size,
            CopyPixelOperation.SourceCopy);

        var lockRect = new Rectangle(0, 0, captureContext.SourceWidth, captureContext.SourceHeight);
        var bitmapData = captureContext.Bitmap.LockBits(lockRect, ImageLockMode.ReadOnly, PixelFormat.Format32bppArgb);
        try
        {
            var rowBytes = captureContext.SourceWidth * 4;
            var totalBytes = rowBytes * captureContext.SourceHeight;
            var managed = new byte[totalBytes];
            for (var y = 0; y < captureContext.SourceHeight; y++)
            {
                var sourceRow = IntPtr.Add(bitmapData.Scan0, y * bitmapData.Stride);
                Marshal.Copy(sourceRow, managed, y * rowBytes, rowBytes);
            }

            return managed;
        }
        finally
        {
            captureContext.Bitmap.UnlockBits(bitmapData);
        }
    }

    private SenderEncoderContext CreateEncoder(int sourceWidth, int sourceHeight)
    {
        var failures = new List<string>();
        foreach (var plan in EncoderPlans)
        {
            try
            {
                var encoder = CreateEncoderForPlan(sourceWidth, sourceHeight, plan);
                ReceiverTrace.Log($"Windows sender encoder selected: {plan.Name}");
                return encoder;
            }
            catch (Exception ex)
            {
                ReceiverTrace.Log(ex, $"Windows sender encoder plan failed: {plan.Name}");
                failures.Add($"{plan.Name}: {ex.Message}");
            }
        }

        throw new InvalidOperationException(
            $"No Media Foundation {_selectedCodec.ToUiLabel()} encoder path available. Tried: " + string.Join(" | ", failures));
    }

    private SenderEncoderContext CreateEncoderForPlan(int sourceWidth, int sourceHeight, EncoderPlan plan)
    {
        var callback = new SampleGrabberCallback(OnEncodedSample);
        IMFActivate? activate = null;
        IMFMediaSink? mediaSink = null;
        IMFAttributes? writerAttributes = null;
        IMFMediaType? outputType = null;
        IMFMediaType? inputType = null;
        IMFSinkWriter? sinkWriter = null;

        try
        {
            outputType = MediaFactory.MFCreateMediaType();
            outputType.Set(MediaTypeAttributeKeys.MajorType, MediaTypeGuids.Video).CheckError();
            outputType.Set(MediaTypeAttributeKeys.Subtype, _selectedCodec.ToMediaFoundationSubtype()).CheckError();
            MediaFactory.MFSetAttributeSize(outputType, MediaTypeAttributeKeys.FrameSize, (uint)_targetWidth, (uint)_targetHeight).CheckError();
            MediaFactory.MFSetAttributeSize(outputType, MediaTypeAttributeKeys.FrameRate, (uint)Math.Max(1, _targetFps), 1).CheckError();
            MediaFactory.MFSetAttributeSize(outputType, MediaTypeAttributeKeys.PixelAspectRatio, 1, 1).CheckError();
            outputType.Set(MediaTypeAttributeKeys.AvgBitrate, (uint)Math.Max(1, _targetBitrateBps)).CheckError();
            outputType.Set(MediaTypeAttributeKeys.InterlaceMode, (uint)VideoInterlaceMode.Progressive).CheckError();

            activate = MediaFactory.MFCreateSampleGrabberSinkActivate(outputType, callback);
            activate.Set(SampleGrabberSinkAttributeKeys.IgnoreClock, true).CheckError();
            mediaSink = activate.ActivateObject<IMFMediaSink>();

            writerAttributes = MediaFactory.MFCreateAttributes(8);
            writerAttributes.Set(SinkWriterAttributeKeys.DisableThrottling, true).CheckError();
            writerAttributes.Set(SinkWriterAttributeKeys.LowLatency, true).CheckError();
            if (plan.HardwareTransforms)
            {
                writerAttributes.Set(SinkWriterAttributeKeys.ReadwriteEnableHardwareTransforms, true).CheckError();
            }

            sinkWriter = MediaFactory.MFCreateSinkWriterFromMediaSink(mediaSink, writerAttributes);
            using var streamSink = mediaSink.GetStreamSinkByIndex(0);
            var streamIndex = streamSink.Identifier;

            inputType = MediaFactory.MFCreateMediaType();
            inputType.Set(MediaTypeAttributeKeys.MajorType, MediaTypeGuids.Video).CheckError();
            inputType.Set(MediaTypeAttributeKeys.Subtype, plan.InputSubtype).CheckError();
            MediaFactory.MFSetAttributeSize(inputType, MediaTypeAttributeKeys.FrameSize, (uint)sourceWidth, (uint)sourceHeight).CheckError();
            MediaFactory.MFSetAttributeSize(inputType, MediaTypeAttributeKeys.FrameRate, (uint)Math.Max(1, _targetFps), 1).CheckError();
            MediaFactory.MFSetAttributeSize(inputType, MediaTypeAttributeKeys.PixelAspectRatio, 1, 1).CheckError();
            inputType.Set(MediaTypeAttributeKeys.InterlaceMode, (uint)VideoInterlaceMode.Progressive).CheckError();
            inputType.Set(MediaTypeAttributeKeys.DefaultStride, (uint)Math.Max(0, sourceWidth * 4)).CheckError();
            inputType.Set(MediaTypeAttributeKeys.SampleSize, (uint)Math.Max(0, sourceWidth * sourceHeight * 4)).CheckError();

            sinkWriter.SetInputMediaType(streamIndex, inputType, null);
            sinkWriter.BeginWriting();

            return new SenderEncoderContext(
                callback,
                activate,
                mediaSink,
                writerAttributes,
                outputType,
                inputType,
                sinkWriter,
                streamIndex);
        }
        catch
        {
            sinkWriter?.Dispose();
            inputType?.Dispose();
            outputType?.Dispose();
            writerAttributes?.Dispose();
            mediaSink?.Dispose();
            if (activate is not null)
            {
                try
                {
                    activate.ShutdownObject();
                }
                catch
                {
                }

                activate.Dispose();
            }

            callback.Dispose();
            throw;
        }
    }

    private void OnEncodedSample(long sampleTimeHns, byte[] rawPayload)
    {
        try
        {
            if (rawPayload.Length == 0 || _udpClient is null)
            {
                return;
            }

            ProcessEncodedPayload(rawPayload, Math.Max(0, sampleTimeHns / 10), annexBInput: false);
            UpdateEncodeStats(Stopwatch.GetTimestamp());
        }
        catch (Exception ex)
        {
            ReceiverTrace.Log(ex, "Windows sender sample callback failed");
            lock (_sync)
            {
                _snapshot = _snapshot with
                {
                    Status = $"Encode callback error: {ex.Message}",
                    LastEncoderError = ex.Message,
                };
            }
        }
    }

    private void ProcessEncodedPayload(byte[] payload, long presentationTimeUs, bool annexBInput)
    {
        byte[] codecConfig;
        byte[] framePayload;
        bool isKeyFrame;
        if (_selectedCodec.IsAv1())
        {
            codecConfig = Array.Empty<byte>();
            framePayload = payload;
            isKeyFrame = true;
        }
        else
        {
            var annexB = annexBInput ? payload : NormalizeToAnnexB(payload);
            codecConfig = ExtractCodecConfig(annexB, _selectedCodec);
            framePayload = StripCodecConfigNalUnits(annexB, _selectedCodec);
            isKeyFrame = ContainsKeyFrame(framePayload, _selectedCodec);
        }
        var nowTicks = Stopwatch.GetTimestamp();
        PendingLatencyPulse? pendingPulse = null;
        if (codecConfig.Length > 0 && (_lastCodecConfig is null || !_lastCodecConfig.AsSpan().SequenceEqual(codecConfig)))
        {
            _lastCodecConfig = codecConfig;
            SendPacket(_packetizer.BuildCodecConfigPacket(codecConfig), TransportProtocol.TypeCodecConfig);
        }

        if (framePayload.Length == 0)
        {
            return;
        }

        lock (_sync)
        {
            if (_pendingLatencyPulse is not null)
            {
                pendingPulse = _pendingLatencyPulse;
                _pendingLatencyPulse = null;
            }
        }

        if (pendingPulse is not null)
        {
            var senderPipelineMs = (int)Math.Clamp(
                Math.Round(Stopwatch.GetElapsedTime(pendingPulse.TriggerTicks, nowTicks).TotalMilliseconds),
                0,
                10_000);
            var pulsePacket = ControlPacketBuilder.BuildLatencyPulse(
                pulseId: pendingPulse.PulseId,
                source: pendingPulse.Source,
                presentationTimeUs: presentationTimeUs,
                tapToUiMs: 0,
                senderPipelineMs: senderPipelineMs,
                approxSenderMs: senderPipelineMs,
                inputSeq: pendingPulse.InputSeq);
            SendPacket(pulsePacket, TransportProtocol.TypeControl);
            lock (_sync)
            {
                _snapshot = _snapshot with { LastControlKind = "latency_pulse_sent" };
            }
        }

        var frameId = Interlocked.Increment(ref _nextFrameId);
        foreach (var packet in _packetizer.PacketizeVideoFrame(frameId, presentationTimeUs, isKeyFrame, framePayload))
        {
            SendPacket(packet, TransportProtocol.TypeVideoFrame);
        }
    }

    private async Task RunFfmpegSoftwareCaptureLoopAsync(WindowsCaptureTargetInfo target, string ffmpegPath, CancellationToken cancellationToken)
    {
        _lastCodecConfig = null;
        _nextFrameId = 1;

        using var process = new Process
        {
            StartInfo = BuildFfmpegStartInfo(ffmpegPath, target),
        };

        if (!process.Start())
        {
            throw new InvalidOperationException("Failed to start ffmpeg software sender fallback");
        }

        TryRaiseProcessPriority(process, ProcessPriorityClass.High);
        ReceiverTrace.Log($"FFmpeg software sender started: {ffmpegPath}");
        SendSessionConfig();
        _lastEncoderStartTicks = Stopwatch.GetTimestamp();
        lock (_sync)
        {
            _snapshot = _snapshot with
            {
                Status = $"Streaming {target.UiLabel} -> {GetTransportTargetLabel()} (FFmpeg { _selectedCodec.ToFfmpegEncoderName()} fallback)",
                EncoderPath = WindowsSenderEncoderBackend.FfmpegSoftware.ToPathLabel(),
                LastEncoderError = "-",
            };
        }

        using var cancellationRegistration = cancellationToken.Register(() => TryTerminateProcess(process));
        var stderrTask = process.StandardError.ReadToEndAsync();
        var reader = CreateEncodedAccessUnitReader();
        var startedAtTicks = Stopwatch.GetTimestamp();
        long lastPresentationTimeUs = 0;
        var buffer = ArrayPool<byte>.Shared.Rent(64 * 1024);

        try
        {
            while (!cancellationToken.IsCancellationRequested)
            {
                var bytesRead = await process.StandardOutput.BaseStream.ReadAsync(buffer.AsMemory(0, buffer.Length), cancellationToken);
                if (bytesRead <= 0)
                {
                    break;
                }

                reader.Feed(buffer.AsSpan(0, bytesRead), (bytes, isKeyFrame) =>
                {
                    var nowTicks = Stopwatch.GetTimestamp();
                    var presentationTimeUs = Math.Max(
                        lastPresentationTimeUs + 1,
                        (long)Math.Round(Stopwatch.GetElapsedTime(startedAtTicks).TotalMilliseconds * 1_000.0));
                    lastPresentationTimeUs = presentationTimeUs;
                    ProcessEncodedPayload(bytes, presentationTimeUs, annexBInput: true);
                    UpdateCaptureStats(nowTicks, 0);
                    UpdateEncodeStats(nowTicks);
                });
            }

            reader.Complete((bytes, isKeyFrame) =>
            {
                var nowTicks = Stopwatch.GetTimestamp();
                var presentationTimeUs = Math.Max(
                    lastPresentationTimeUs + 1,
                    (long)Math.Round(Stopwatch.GetElapsedTime(startedAtTicks).TotalMilliseconds * 1_000.0));
                lastPresentationTimeUs = presentationTimeUs;
                ProcessEncodedPayload(bytes, presentationTimeUs, annexBInput: true);
                UpdateCaptureStats(nowTicks, 0);
                UpdateEncodeStats(nowTicks);
            });

            if (cancellationToken.IsCancellationRequested)
            {
                return;
            }

            var stderr = await stderrTask;
            if (!process.WaitForExit(500))
            {
                throw new IOException("ffmpeg software sender stopped responding");
            }

            var errorMessage = string.IsNullOrWhiteSpace(stderr)
                ? $"ffmpeg exited with code {process.ExitCode}"
                : stderr.Trim();
            throw new IOException(errorMessage);
        }
        finally
        {
            ArrayPool<byte>.Shared.Return(buffer);
            TryTerminateProcess(process);
        }
    }

    private ProcessStartInfo BuildFfmpegStartInfo(string ffmpegPath, WindowsCaptureTargetInfo target)
    {
        var gop = Math.Max(1, _targetFps * Math.Max(1, _senderSpec.KeyFrameIntervalSeconds));
        var bitrateKbps = Math.Max(100, _targetBitrateBps / 1000);
        var vbvBufferKbps = ComputeLowLatencyVbvBufferKbps(bitrateKbps);
        var rtBufferSize = ComputeCaptureRtBufferSize(target.Bounds.Size);
        var scaleFilter = BuildSoftwareScaleFilter();
        var startInfo = new ProcessStartInfo
        {
            FileName = ffmpegPath,
            UseShellExecute = false,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            CreateNoWindow = true,
        };

        startInfo.ArgumentList.Add("-hide_banner");
        startInfo.ArgumentList.Add("-loglevel");
        startInfo.ArgumentList.Add("warning");
        startInfo.ArgumentList.Add("-thread_queue_size");
        startInfo.ArgumentList.Add("1");
        startInfo.ArgumentList.Add("-fflags");
        startInfo.ArgumentList.Add("nobuffer+discardcorrupt");
        startInfo.ArgumentList.Add("-flags");
        startInfo.ArgumentList.Add("low_delay");
        startInfo.ArgumentList.Add("-avioflags");
        startInfo.ArgumentList.Add("direct");
        startInfo.ArgumentList.Add("-probesize");
        startInfo.ArgumentList.Add("32");
        startInfo.ArgumentList.Add("-analyzeduration");
        startInfo.ArgumentList.Add("0");
        startInfo.ArgumentList.Add("-rtbufsize");
        startInfo.ArgumentList.Add(rtBufferSize);
        startInfo.ArgumentList.Add("-use_wallclock_as_timestamps");
        startInfo.ArgumentList.Add("1");
        startInfo.ArgumentList.Add("-f");
        startInfo.ArgumentList.Add("gdigrab");
        startInfo.ArgumentList.Add("-framerate");
        startInfo.ArgumentList.Add(_targetFps.ToString(CultureInfo.InvariantCulture));
        startInfo.ArgumentList.Add("-offset_x");
        startInfo.ArgumentList.Add(target.Bounds.X.ToString(CultureInfo.InvariantCulture));
        startInfo.ArgumentList.Add("-offset_y");
        startInfo.ArgumentList.Add(target.Bounds.Y.ToString(CultureInfo.InvariantCulture));
        startInfo.ArgumentList.Add("-video_size");
        startInfo.ArgumentList.Add($"{target.Bounds.Width}x{target.Bounds.Height}");
        startInfo.ArgumentList.Add("-draw_mouse");
        startInfo.ArgumentList.Add(_captureCursorInStream ? "1" : "0");
        startInfo.ArgumentList.Add("-i");
        startInfo.ArgumentList.Add("desktop");
        startInfo.ArgumentList.Add("-an");
        startInfo.ArgumentList.Add("-sn");
        startInfo.ArgumentList.Add("-fps_mode");
        startInfo.ArgumentList.Add("passthrough");
        startInfo.ArgumentList.Add("-vf");
        startInfo.ArgumentList.Add(scaleFilter);
        startInfo.ArgumentList.Add("-c:v");
        startInfo.ArgumentList.Add(_selectedCodec.ToFfmpegEncoderName());
        startInfo.ArgumentList.Add("-preset");
        startInfo.ArgumentList.Add(_selectedCodec.IsAv1() ? "8" : "ultrafast");
        if (!_selectedCodec.IsAv1())
        {
            startInfo.ArgumentList.Add("-tune");
            startInfo.ArgumentList.Add("zerolatency");
        }
        startInfo.ArgumentList.Add("-pix_fmt");
        startInfo.ArgumentList.Add("yuv420p");
        if (!_selectedCodec.IsHevc() && !_selectedCodec.IsAv1())
        {
            startInfo.ArgumentList.Add("-profile:v");
            startInfo.ArgumentList.Add("baseline");
        }
        startInfo.ArgumentList.Add("-g");
        startInfo.ArgumentList.Add(gop.ToString(CultureInfo.InvariantCulture));
        startInfo.ArgumentList.Add("-keyint_min");
        startInfo.ArgumentList.Add(gop.ToString(CultureInfo.InvariantCulture));
        startInfo.ArgumentList.Add("-sc_threshold");
        startInfo.ArgumentList.Add("0");
        startInfo.ArgumentList.Add("-bf");
        startInfo.ArgumentList.Add("0");
        startInfo.ArgumentList.Add("-b:v");
        startInfo.ArgumentList.Add($"{bitrateKbps}k");
        startInfo.ArgumentList.Add("-maxrate");
        startInfo.ArgumentList.Add($"{bitrateKbps}k");
        startInfo.ArgumentList.Add("-bufsize");
        startInfo.ArgumentList.Add($"{vbvBufferKbps}k");
        if (_selectedCodec.IsAv1())
        {
            startInfo.ArgumentList.Add("-svtav1-params");
            startInfo.ArgumentList.Add($"keyint={gop}:hierarchical-levels=0:scd=0:enable-overlays=0:tune=0");
        }
        startInfo.ArgumentList.Add("-flush_packets");
        startInfo.ArgumentList.Add("1");
        startInfo.ArgumentList.Add("-f");
        startInfo.ArgumentList.Add(_selectedCodec.ToFfmpegMuxerName());
        startInfo.ArgumentList.Add("-");
        return startInfo;
    }

    private ProcessStartInfo BuildFfmpegHardwareCaptureStartInfo(
        string ffmpegPath,
        WindowsCaptureTargetInfo target,
        WindowsSenderEncoderBackend encoderBackend,
        FfmpegCaptureInputBackend captureBackend)
    {
        var gamePreset = IsGamePreset();
        var gop = Math.Max(1, _targetFps * Math.Max(1, _senderSpec.KeyFrameIntervalSeconds));
        var bitrateKbps = Math.Max(100, _targetBitrateBps / 1000);
        var vbvBufferKbps = ComputeLowLatencyVbvBufferKbps(bitrateKbps);
        var rtBufferSize = ComputeCaptureRtBufferSize(target.Bounds.Size);
        var scaleFilter = BuildHardwareScaleFilter(encoderBackend, captureBackend);
        var encoderName = encoderBackend switch
        {
            WindowsSenderEncoderBackend.NvidiaNvenc => _selectedCodec switch
            {
                WindowsVideoCodec.H265Hevc => "hevc_nvenc",
                WindowsVideoCodec.Av1 => "av1_nvenc",
                _ => "h264_nvenc",
            },
            WindowsSenderEncoderBackend.IntelQuickSync => _selectedCodec switch
            {
                WindowsVideoCodec.H265Hevc => "hevc_qsv",
                WindowsVideoCodec.Av1 => "av1_qsv",
                _ => "h264_qsv",
            },
            _ => throw new ArgumentOutOfRangeException(nameof(encoderBackend), encoderBackend, null),
        };

        var startInfo = new ProcessStartInfo
        {
            FileName = ffmpegPath,
            UseShellExecute = false,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            CreateNoWindow = true,
        };

        startInfo.ArgumentList.Add("-hide_banner");
        startInfo.ArgumentList.Add("-loglevel");
        startInfo.ArgumentList.Add("warning");
        startInfo.ArgumentList.Add("-thread_queue_size");
        startInfo.ArgumentList.Add("1");
        startInfo.ArgumentList.Add("-fflags");
        startInfo.ArgumentList.Add("nobuffer+discardcorrupt");
        startInfo.ArgumentList.Add("-flags");
        startInfo.ArgumentList.Add("low_delay");
        startInfo.ArgumentList.Add("-avioflags");
        startInfo.ArgumentList.Add("direct");
        startInfo.ArgumentList.Add("-probesize");
        startInfo.ArgumentList.Add("32");
        startInfo.ArgumentList.Add("-analyzeduration");
        startInfo.ArgumentList.Add("0");
        startInfo.ArgumentList.Add("-rtbufsize");
        startInfo.ArgumentList.Add(rtBufferSize);
        startInfo.ArgumentList.Add("-use_wallclock_as_timestamps");
        startInfo.ArgumentList.Add("1");

        switch (captureBackend)
        {
            case FfmpegCaptureInputBackend.DdaGrab:
                if (!TryResolveDxgiOutputIndex(target.DeviceName, out var outputIndex))
                {
                    throw new InvalidOperationException($"Desktop Duplication output index not found for {target.DeviceName}");
                }

                startInfo.ArgumentList.Add("-f");
                startInfo.ArgumentList.Add("lavfi");
                startInfo.ArgumentList.Add("-i");
                startInfo.ArgumentList.Add(
                    $"ddagrab=output_idx={outputIndex}:draw_mouse={(_captureCursorInStream ? 1 : 0)}:framerate={_targetFps}:video_size={target.Bounds.Width}x{target.Bounds.Height}:offset_x=0:offset_y=0:output_fmt=bgra:allow_fallback=1:dup_frames={(IsGamePreset() ? 0 : 1)}");
                break;

            case FfmpegCaptureInputBackend.GdiGrab:
                startInfo.ArgumentList.Add("-f");
                startInfo.ArgumentList.Add("gdigrab");
                startInfo.ArgumentList.Add("-framerate");
                startInfo.ArgumentList.Add(_targetFps.ToString(CultureInfo.InvariantCulture));
                startInfo.ArgumentList.Add("-offset_x");
                startInfo.ArgumentList.Add(target.Bounds.X.ToString(CultureInfo.InvariantCulture));
                startInfo.ArgumentList.Add("-offset_y");
                startInfo.ArgumentList.Add(target.Bounds.Y.ToString(CultureInfo.InvariantCulture));
                startInfo.ArgumentList.Add("-video_size");
                startInfo.ArgumentList.Add($"{target.Bounds.Width}x{target.Bounds.Height}");
                startInfo.ArgumentList.Add("-draw_mouse");
                startInfo.ArgumentList.Add(_captureCursorInStream ? "1" : "0");
                startInfo.ArgumentList.Add("-i");
                startInfo.ArgumentList.Add("desktop");
                break;
        }

        startInfo.ArgumentList.Add("-an");
        startInfo.ArgumentList.Add("-sn");
        startInfo.ArgumentList.Add("-fps_mode");
        startInfo.ArgumentList.Add("passthrough");
        startInfo.ArgumentList.Add("-vf");
        startInfo.ArgumentList.Add(scaleFilter);
        startInfo.ArgumentList.Add("-c:v");
        startInfo.ArgumentList.Add(encoderName);
        if (!_selectedCodec.IsAv1())
        {
            startInfo.ArgumentList.Add("-profile:v");
            startInfo.ArgumentList.Add(_selectedCodec.IsHevc() ? "main" : "high");
        }

        switch (encoderBackend)
        {
            case WindowsSenderEncoderBackend.NvidiaNvenc:
                startInfo.ArgumentList.Add("-preset");
                startInfo.ArgumentList.Add(gamePreset ? "p1" : "llhp");
                if (!_selectedCodec.IsAv1())
                {
                    startInfo.ArgumentList.Add("-tune");
                    startInfo.ArgumentList.Add(gamePreset ? "ull" : "ll");
                }
                startInfo.ArgumentList.Add("-rc");
                startInfo.ArgumentList.Add("cbr");
                startInfo.ArgumentList.Add("-rc-lookahead");
                startInfo.ArgumentList.Add("0");
                if (!_selectedCodec.IsAv1())
                {
                    startInfo.ArgumentList.Add("-zerolatency");
                    startInfo.ArgumentList.Add("1");
                }
                startInfo.ArgumentList.Add("-delay");
                startInfo.ArgumentList.Add("0");
                if (!_selectedCodec.IsAv1())
                {
                    startInfo.ArgumentList.Add("-aud");
                    startInfo.ArgumentList.Add("1");
                }
                if (gamePreset)
                {
                    startInfo.ArgumentList.Add("-nonref_p");
                    startInfo.ArgumentList.Add("1");
                }
                startInfo.ArgumentList.Add("-surfaces");
                startInfo.ArgumentList.Add("2");
                startInfo.ArgumentList.Add("-strict_gop");
                startInfo.ArgumentList.Add("1");
                startInfo.ArgumentList.Add("-forced-idr");
                startInfo.ArgumentList.Add("1");
                break;

            case WindowsSenderEncoderBackend.IntelQuickSync:
                if (gamePreset)
                {
                    startInfo.ArgumentList.Add("-preset");
                    startInfo.ArgumentList.Add("veryfast");
                    if (!_selectedCodec.IsAv1())
                    {
                        startInfo.ArgumentList.Add("-low_power");
                        startInfo.ArgumentList.Add("1");
                    }
                    startInfo.ArgumentList.Add("-low_delay_brc");
                    startInfo.ArgumentList.Add("1");
                    startInfo.ArgumentList.Add("-scenario");
                    startInfo.ArgumentList.Add("remotegaming");
                }
                startInfo.ArgumentList.Add("-look_ahead");
                startInfo.ArgumentList.Add("0");
                startInfo.ArgumentList.Add("-async_depth");
                startInfo.ArgumentList.Add("1");
                if (!_selectedCodec.IsHevc() && !_selectedCodec.IsAv1())
                {
                    startInfo.ArgumentList.Add("-aud");
                    startInfo.ArgumentList.Add("1");
                    startInfo.ArgumentList.Add("-repeat_pps");
                    startInfo.ArgumentList.Add("1");
                }
                break;
        }

        startInfo.ArgumentList.Add("-g");
        startInfo.ArgumentList.Add(gop.ToString(CultureInfo.InvariantCulture));
        startInfo.ArgumentList.Add("-keyint_min");
        startInfo.ArgumentList.Add(gop.ToString(CultureInfo.InvariantCulture));
        startInfo.ArgumentList.Add("-bf");
        startInfo.ArgumentList.Add("0");
        startInfo.ArgumentList.Add("-b:v");
        startInfo.ArgumentList.Add($"{bitrateKbps}k");
        startInfo.ArgumentList.Add("-maxrate");
        startInfo.ArgumentList.Add($"{bitrateKbps}k");
        startInfo.ArgumentList.Add("-bufsize");
        startInfo.ArgumentList.Add($"{vbvBufferKbps}k");
        startInfo.ArgumentList.Add("-flush_packets");
        startInfo.ArgumentList.Add("1");
        startInfo.ArgumentList.Add("-f");
        startInfo.ArgumentList.Add(_selectedCodec.ToFfmpegMuxerName());
        startInfo.ArgumentList.Add("-");
        return startInfo;
    }

    private string BuildSoftwareScaleFilter()
    {
        var scaleFlags = ShouldPrioritizeLatency() ? "fast_bilinear" : "bicubic";
        return $"scale={_targetWidth}:{_targetHeight}:flags={scaleFlags},format=yuv420p";
    }

    private string BuildHardwareScaleFilter(WindowsSenderEncoderBackend encoderBackend, FfmpegCaptureInputBackend captureBackend)
    {
        var softwareScaleFlags = ShouldPrioritizeLatency() ? "fast_bilinear" : "bicubic";
        return encoderBackend switch
        {
            WindowsSenderEncoderBackend.NvidiaNvenc => captureBackend == FfmpegCaptureInputBackend.DdaGrab
                ? $"hwdownload,format=bgra,scale={_targetWidth}:{_targetHeight}:flags={softwareScaleFlags},format=nv12,hwupload_cuda"
                : $"scale={_targetWidth}:{_targetHeight}:flags={softwareScaleFlags},format=nv12,hwupload_cuda",
            WindowsSenderEncoderBackend.IntelQuickSync => captureBackend == FfmpegCaptureInputBackend.DdaGrab
                ? $"hwdownload,format=bgra,scale={_targetWidth}:{_targetHeight}:flags={softwareScaleFlags},format=nv12"
                : $"scale={_targetWidth}:{_targetHeight}:flags={softwareScaleFlags},format=nv12",
            _ => $"scale={_targetWidth}:{_targetHeight}:flags=bicubic,format=nv12",
        };
    }

    private static bool TryResolveDxgiOutputIndex(string deviceName, out int outputIndex)
    {
        outputIndex = -1;
        using var factory = DXGI.CreateDXGIFactory1<IDXGIFactory1>();
        var globalIndex = 0;

        for (uint adapterIndex = 0; ; adapterIndex++)
        {
            var adapterResult = factory.EnumAdapters1(adapterIndex, out var adapter);
            if (adapterResult.Failure || adapter is null)
            {
                break;
            }

            using (adapter)
            {
                for (uint localOutputIndex = 0; ; localOutputIndex++)
                {
                    var outputResult = adapter.EnumOutputs(localOutputIndex, out var output);
                    if (outputResult.Failure || output is null)
                    {
                        break;
                    }

                    using (output)
                    {
                        if (string.Equals(output.Description.DeviceName, deviceName, StringComparison.OrdinalIgnoreCase))
                        {
                            outputIndex = globalIndex;
                            return true;
                        }
                    }

                    globalIndex++;
                }
            }
        }

        return false;
    }

    private static string FormatCaptureBackendLabel(FfmpegCaptureInputBackend captureBackend) =>
        captureBackend switch
        {
            FfmpegCaptureInputBackend.DdaGrab => "DDAgrab",
            _ => "GDIgrab",
        };

    private bool ShouldPrioritizeLatency() => IsGamePreset();

    private bool IsGamePreset() =>
        string.Equals(_senderSpec.ProtocolPreset, "GAME", StringComparison.OrdinalIgnoreCase);

    private void ApplySenderSpec(WindowsSenderPresetSpec spec, Size sourceSize)
    {
        _senderSpec = spec;
        var scaledSize = ScaleToFit(sourceSize, new Size(spec.TargetWidth, spec.TargetHeight));
        _targetWidth = scaledSize.Width;
        _targetHeight = scaledSize.Height;
        _targetBitrateBps = spec.TargetBitrateBps;
        _targetFps = spec.TargetFps;
        _sampleDurationHns = Math.Max(1, 10_000_000L / Math.Max(1, spec.TargetFps));
    }

    private void ApplyPendingAdaptiveStepIfAny(Size sourceSize)
    {
        int stepToApply;
        lock (_sync)
        {
            if (_pendingAdaptiveStep < 0 || _pendingAdaptiveStep == _adaptiveStep)
            {
                return;
            }

            stepToApply = _pendingAdaptiveStep;
            _pendingAdaptiveStep = -1;
        }

        var adaptedSpec = ComputeAdaptiveSpec(_baseSenderSpec, stepToApply);
        ApplySenderSpec(adaptedSpec, sourceSize);
        _adaptiveStep = stepToApply;
        _lastAdaptiveChangeTicks = Stopwatch.GetTimestamp();

        lock (_sync)
        {
            _snapshot = _snapshot with
            {
                Resolution = $"{_targetWidth}x{_targetHeight}",
                TargetFps = _targetFps,
                BitrateMbps = _targetBitrateBps / 1_000_000.0,
                AdaptiveEnabled = _adaptiveEnabled,
                AdaptiveStep = _adaptiveStep,
                LastControlKind = $"adaptive_step:{_adaptiveStep}",
            };
        }
    }

    private void ConsiderAdaptiveRelief(ReceiverFeedbackControl feedback)
    {
        if (!_adaptiveEnabled)
        {
            return;
        }

        var nowTicks = Stopwatch.GetTimestamp();
        var sinceEncoderStartMs = _lastEncoderStartTicks <= 0
            ? long.MaxValue
            : (long)Stopwatch.GetElapsedTime(_lastEncoderStartTicks, nowTicks).TotalMilliseconds;
        var sinceLastChangeMs = _lastAdaptiveChangeTicks <= 0
            ? long.MaxValue
            : (long)Stopwatch.GetElapsedTime(_lastAdaptiveChangeTicks, nowTicks).TotalMilliseconds;
        if (sinceEncoderStartMs < 1800 || sinceLastChangeMs < AdaptiveChangeCooldownMs)
        {
            return;
        }

        var pressureCritical = string.Equals(feedback.Pressure, "critical", StringComparison.OrdinalIgnoreCase);
        var pressureHigh = string.Equals(feedback.Pressure, "high", StringComparison.OrdinalIgnoreCase);
        var decodeBehind = feedback.DecodeFps > 0 && feedback.DecodeFps <= Math.Max(28, _targetFps - 3);
        var decodeCollapsed = feedback.DecodeFps > 0 && feedback.DecodeFps <= Math.Max(24, _targetFps - 8);
        var presentElevated = feedback.PresentDeltaMs >= 20;
        var presentHigh = feedback.PresentDeltaMs >= 26;
        var strained = pressureCritical || pressureHigh || decodeBehind || presentElevated;
        var severeStrain = pressureCritical || decodeCollapsed || presentHigh;

        if (strained)
        {
            var strainWeight = 0;
            if (pressureCritical)
            {
                strainWeight += 3;
            }
            else if (pressureHigh)
            {
                strainWeight += 2;
            }

            if (decodeBehind)
            {
                strainWeight += decodeCollapsed ? 3 : 2;
            }

            if (presentElevated)
            {
                strainWeight += presentHigh ? 2 : 1;
            }

            _receiverStrainScore += Math.Max(1, strainWeight);
            _receiverRecoveryScore = 0;
        }
        else
        {
            _receiverRecoveryScore += 1;
            _receiverStrainScore = Math.Max(0, _receiverStrainScore - 1);
        }

        var nextStepThreshold = _adaptiveStep switch
        {
            0 => 2,
            _ => severeStrain ? 4 : 6,
        };
        if (_adaptiveStep < 2 && _receiverStrainScore >= nextStepThreshold)
        {
            lock (_sync)
            {
                if (_pendingAdaptiveStep >= 0)
                {
                    return;
                }

                _pendingAdaptiveStep = _adaptiveStep + 1;
            }

            _receiverStrainScore = 0;
            Interlocked.Exchange(ref _forceEncoderRestartRequested, 1);
            return;
        }

        if (_adaptiveStep > 0 &&
            _receiverRecoveryScore >= 10 &&
            string.Equals(feedback.Pressure, "normal", StringComparison.OrdinalIgnoreCase) &&
            (feedback.DecodeFps <= 0 || feedback.DecodeFps >= Math.Max(1, _targetFps - 1)) &&
            feedback.PresentDeltaMs >= 0 &&
            feedback.PresentDeltaMs <= 16 &&
            feedback.InputEstimateMs >= 0 &&
            feedback.InputEstimateMs <= 45)
        {
            lock (_sync)
            {
                if (_pendingAdaptiveStep >= 0)
                {
                    return;
                }

                _pendingAdaptiveStep = _adaptiveStep - 1;
            }

            _receiverRecoveryScore = 0;
            Interlocked.Exchange(ref _forceEncoderRestartRequested, 1);
        }
    }

    private static WindowsSenderPresetSpec ComputeAdaptiveSpec(WindowsSenderPresetSpec baseSpec, int adaptiveStep)
    {
        if (adaptiveStep <= 0)
        {
            return baseSpec;
        }

        var game = string.Equals(baseSpec.ProtocolPreset, "GAME", StringComparison.OrdinalIgnoreCase);
        var scale = adaptiveStep switch
        {
            1 => game ? 0.90 : 0.88,
            _ => game ? 0.84 : 0.80,
        };
        var fps = adaptiveStep switch
        {
            1 => Math.Min(baseSpec.TargetFps, game ? 58 : 55),
            _ => Math.Min(baseSpec.TargetFps, game ? 52 : 48),
        };
        var bitrateScale = adaptiveStep switch
        {
            1 => game ? 0.80 : 0.76,
            _ => game ? 0.70 : 0.62,
        };

        return baseSpec with
        {
            TargetWidth = RoundToEven(Math.Max(64, (int)Math.Round(baseSpec.TargetWidth * scale))),
            TargetHeight = RoundToEven(Math.Max(64, (int)Math.Round(baseSpec.TargetHeight * scale))),
            TargetFps = fps,
            TargetBitrateBps = Math.Max(400_000, (int)Math.Round(baseSpec.TargetBitrateBps * bitrateScale)),
        };
    }

    private static int RoundToEven(int value) => value % 2 == 0 ? value : value - 1;

    private int ComputeLowLatencyVbvBufferKbps(int bitrateKbps)
    {
        var framesBuffered = ShouldPrioritizeLatency()
            ? (_targetFps >= 90 ? 1 : 2)
            : Math.Clamp(_targetFps >= 90 ? 2 : 3, 2, 3);
        var perFrameBudgetKbits = bitrateKbps / (double)Math.Max(1, _targetFps);
        var targetBufferKbits = (int)Math.Ceiling(perFrameBudgetKbits * framesBuffered);
        var minBufferKbits = ShouldPrioritizeLatency()
            ? Math.Min(Math.Max(64, bitrateKbps / 32), bitrateKbps)
            : Math.Min(Math.Max(96, bitrateKbps / 20), bitrateKbps);
        var maxBufferKbits = Math.Max(minBufferKbits, ShouldPrioritizeLatency() ? bitrateKbps / 6 : bitrateKbps / 4);
        return Math.Clamp(targetBufferKbits, minBufferKbits, maxBufferKbits);
    }

    private string ComputeCaptureRtBufferSize(Size captureSize)
    {
        var bytesPerFrame = (long)Math.Max(1, captureSize.Width) * Math.Max(1, captureSize.Height) * 4L;
        var targetBytes = ShouldPrioritizeLatency()
            ? Math.Max(8L * 1024 * 1024, bytesPerFrame)
            : Math.Max(16L * 1024 * 1024, bytesPerFrame * 2L);
        var minMib = ShouldPrioritizeLatency() ? 8 : 16;
        var maxMib = ShouldPrioritizeLatency() ? 64 : 128;
        var mib = (int)Math.Clamp((targetBytes + (1024 * 1024 - 1)) / (1024 * 1024), minMib, maxMib);
        return $"{mib}M";
    }

    private static SenderAudioConfig BuildSenderAudioConfig(WaveFormat waveFormat)
    {
        return new SenderAudioConfig(
            SampleRate: Math.Max(8_000, waveFormat.SampleRate),
            Channels: waveFormat.Channels > 1 ? 2 : 1,
            BytesPerSample: 2);
    }

    private static byte[] ConvertCapturedAudioToPcm16(WaveFormat waveFormat, byte[] buffer, int bytesRecorded, int outputChannels)
    {
        if (bytesRecorded <= 0)
        {
            return Array.Empty<byte>();
        }

        var inputChannels = Math.Max(1, waveFormat.Channels);
        var inputBytesPerSample = Math.Max(1, waveFormat.BlockAlign / inputChannels);
        var frameCount = bytesRecorded / Math.Max(1, waveFormat.BlockAlign);
        if (frameCount <= 0)
        {
            return Array.Empty<byte>();
        }

        var output = new byte[frameCount * outputChannels * sizeof(short)];
        var outputOffset = 0;
        var inputSpan = buffer.AsSpan(0, frameCount * waveFormat.BlockAlign);
        for (var frameIndex = 0; frameIndex < frameCount; frameIndex++)
        {
            var frameOffset = frameIndex * waveFormat.BlockAlign;
            for (var channelIndex = 0; channelIndex < outputChannels; channelIndex++)
            {
                var sourceChannel = Math.Min(channelIndex, inputChannels - 1);
                var sampleOffset = frameOffset + sourceChannel * inputBytesPerSample;
                var sample = ReadPcm16Sample(waveFormat, inputSpan, sampleOffset, inputBytesPerSample);
                System.Buffers.Binary.BinaryPrimitives.WriteInt16LittleEndian(output.AsSpan(outputOffset, sizeof(short)), sample);
                outputOffset += sizeof(short);
            }
        }

        return output;
    }

    private static short ReadPcm16Sample(WaveFormat waveFormat, ReadOnlySpan<byte> input, int offset, int inputBytesPerSample)
    {
        var isFloat =
            waveFormat.Encoding == WaveFormatEncoding.IeeeFloat ||
            (waveFormat.Encoding == WaveFormatEncoding.Extensible && waveFormat.BitsPerSample == 32 && inputBytesPerSample >= 4);
        if (isFloat && inputBytesPerSample >= 4)
        {
            var bits = System.Buffers.Binary.BinaryPrimitives.ReadInt32LittleEndian(input.Slice(offset, 4));
            var sample = Math.Clamp(BitConverter.Int32BitsToSingle(bits), -1.0f, 1.0f);
            return (short)Math.Round(sample * short.MaxValue);
        }

        return inputBytesPerSample switch
        {
            1 => (short)((input[offset] - 128) << 8),
            2 => System.Buffers.Binary.BinaryPrimitives.ReadInt16LittleEndian(input.Slice(offset, 2)),
            3 => (short)(ReadInt24LittleEndian(input, offset) >> 8),
            _ => (short)(System.Buffers.Binary.BinaryPrimitives.ReadInt32LittleEndian(input.Slice(offset, 4)) >> 16),
        };
    }

    private static int ReadInt24LittleEndian(ReadOnlySpan<byte> input, int offset)
    {
        var value = input[offset] | (input[offset + 1] << 8) | (input[offset + 2] << 16);
        if ((value & 0x0080_0000) != 0)
        {
            value |= unchecked((int)0xFF00_0000);
        }

        return value;
    }

    private void SendSessionConfig()
    {
        var payload = JsonSerializer.SerializeToUtf8Bytes(
            new
            {
                codec = _selectedCodec.ToMimeType(),
                preset = _senderSpec.ProtocolPreset,
                adaptationMode = "WINDOWS_PC_SENDER",
                transport = "EVRT_REALTIME_V3_WINDOWS_UDP",
                width = _targetWidth,
                height = _targetHeight,
                fps = _targetFps,
                bitrate = _targetBitrateBps,
                streamMode = "single",
                baseWidth = _targetWidth,
                baseHeight = _targetHeight,
                baseBitrate = _targetBitrateBps,
                enhancementEnabled = false,
                enhancementCodec = (string?)null,
                enhancementMaxWidth = 0,
                enhancementMaxHeight = 0,
                roiMode = "none",
            });
        SendPacket(_packetizer.BuildSessionConfigPacket(payload), TransportProtocol.TypeSessionConfig);
    }

    private void SendAudioConfig(SenderAudioConfig config)
    {
        var payload = JsonSerializer.SerializeToUtf8Bytes(
            new
            {
                codec = "pcm_s16le",
                sampleRate = config.SampleRate,
                channels = config.Channels,
                bytesPerSample = config.BytesPerSample,
            });
        SendPacket(_packetizer.BuildAudioConfigPacket(payload), TransportProtocol.TypeAudioConfig);
    }

    private void SendPacket(byte[] packet, byte packetType)
    {
        var client = _udpClient;
        if (client is null)
        {
            return;
        }

        lock (_udpSendSync)
        {
            client.Send(packet, packet.Length);
        }
        lock (_sync)
        {
            _snapshot = _snapshot with
            {
                PacketsSent = _snapshot.PacketsSent + 1,
                SessionConfigPackets = _snapshot.SessionConfigPackets + (packetType == TransportProtocol.TypeSessionConfig ? 1 : 0),
                CodecConfigPackets = _snapshot.CodecConfigPackets + (packetType == TransportProtocol.TypeCodecConfig ? 1 : 0),
                VideoPackets = _snapshot.VideoPackets + (packetType == TransportProtocol.TypeVideoFrame ? 1 : 0),
                AudioPackets = _snapshot.AudioPackets + (
                    packetType == TransportProtocol.TypeAudioConfig || packetType == TransportProtocol.TypeAudioFrame ? 1 : 0),
                ControlPacketsSent = _snapshot.ControlPacketsSent + (packetType == TransportProtocol.TypeControl ? 1 : 0),
            };
        }
    }

    private static void DisableUdpConnectionReset(Socket socket)
    {
        if (!OperatingSystem.IsWindows())
        {
            return;
        }

        try
        {
            socket.IOControl(
                UdpConnectionResetIoControlCode,
                new byte[] { 0, 0, 0, 0 },
                null);
        }
        catch
        {
        }
    }

    private void UpdateAudioStatus(string status)
    {
        lock (_sync)
        {
            _snapshot = _snapshot with { AudioStatus = status };
        }
    }

    private void UpdateCaptureStats(long nowTicks, uint droppedFrames)
    {
        lock (_sync)
        {
            _captureTicks.Enqueue(nowTicks);
            TrimTickQueue(_captureTicks, nowTicks);
            _snapshot = _snapshot with
            {
                FramesCaptured = _snapshot.FramesCaptured + 1,
                FramesDropped = _snapshot.FramesDropped + droppedFrames,
                CaptureFps = CalculateFps(_captureTicks, nowTicks),
            };
        }
    }

    private void UpdateEncodeStats(long nowTicks)
    {
        lock (_sync)
        {
            _encodeTicks.Enqueue(nowTicks);
            TrimTickQueue(_encodeTicks, nowTicks);
            _snapshot = _snapshot with
            {
                FramesEncoded = _snapshot.FramesEncoded + 1,
                EncodeFps = CalculateFps(_encodeTicks, nowTicks),
            };
        }
    }

    private void UpdateSubmitStats(long nowTicks)
    {
        lock (_sync)
        {
            _submitTicks.Enqueue(nowTicks);
            TrimTickQueue(_submitTicks, nowTicks);
            _snapshot = _snapshot with
            {
                SubmitFps = CalculateFps(_submitTicks, nowTicks),
            };
        }
    }

    private void IncrementDroppedFrames()
    {
        lock (_sync)
        {
            _snapshot = _snapshot with { FramesDropped = _snapshot.FramesDropped + 1 };
        }
    }

    private void IncrementNativeDxgiTimeout()
    {
        lock (_sync)
        {
            _snapshot = _snapshot with { NativeDxgiTimeouts = _snapshot.NativeDxgiTimeouts + 1 };
        }
    }

    private void IncrementNativePacedSkip()
    {
        lock (_sync)
        {
            _snapshot = _snapshot with { NativePacedSkips = _snapshot.NativePacedSkips + 1 };
        }
    }

    private static void TrimTickQueue(Queue<long> queue, long nowTicks)
    {
        var minTicks = nowTicks - Stopwatch.Frequency;
        while (queue.Count > 1 && queue.Peek() < minTicks)
        {
            queue.Dequeue();
        }
    }

    private static int CalculateFps(Queue<long> queue, long nowTicks)
    {
        if (queue.Count <= 1)
        {
            return queue.Count;
        }

        var firstTick = queue.Peek();
        var seconds = (nowTicks - firstTick) / (double)Stopwatch.Frequency;
        return seconds <= 0.0 ? queue.Count : Math.Max(1, (int)Math.Round((queue.Count - 1) / seconds));
    }

    private void UpdateNativeStageStats(double acquireWaitMs, double acquireProcessMs, double encodeCallMs, double drainPacketizeMs)
    {
        const double alpha = 0.15;
        _nativeAcquireWaitMsEwma = _nativeAcquireWaitMsEwma <= 0 ? acquireWaitMs : (_nativeAcquireWaitMsEwma * (1.0 - alpha) + acquireWaitMs * alpha);
        _nativeAcquireProcessMsEwma = _nativeAcquireProcessMsEwma <= 0 ? acquireProcessMs : (_nativeAcquireProcessMsEwma * (1.0 - alpha) + acquireProcessMs * alpha);
        _nativeEncodeCallMsEwma = _nativeEncodeCallMsEwma <= 0 ? encodeCallMs : (_nativeEncodeCallMsEwma * (1.0 - alpha) + encodeCallMs * alpha);
        _nativeDrainPacketizeMsEwma = _nativeDrainPacketizeMsEwma <= 0 ? drainPacketizeMs : (_nativeDrainPacketizeMsEwma * (1.0 - alpha) + drainPacketizeMs * alpha);

        lock (_sync)
        {
            _snapshot = _snapshot with
            {
                NativeStageStats =
                    $"wait {Math.Round(_nativeAcquireWaitMsEwma, 2):0.00} ms | " +
                    $"prep {Math.Round(_nativeAcquireProcessMsEwma, 2):0.00} ms | " +
                    $"enc {Math.Round(_nativeEncodeCallMsEwma, 2):0.00} ms | " +
                    $"drain {Math.Round(_nativeDrainPacketizeMsEwma, 2):0.00} ms",
            };
        }
    }

    private static bool TryResolveFfmpegExecutable(out string ffmpegPath)
    {
        static IEnumerable<string> EnumerateCandidates()
        {
            var explicitPath = Environment.GetEnvironmentVariable("EVERTY_FFMPEG_PATH");
            if (!string.IsNullOrWhiteSpace(explicitPath))
            {
                yield return explicitPath.Trim();
            }

            var baseDirectory = AppContext.BaseDirectory;
            if (!string.IsNullOrWhiteSpace(baseDirectory))
            {
                yield return Path.Combine(baseDirectory, "ffmpeg.exe");
                yield return Path.Combine(baseDirectory, "ffmpeg", "ffmpeg.exe");
                yield return Path.Combine(baseDirectory, "ffmpeg", "bin", "ffmpeg.exe");
                yield return Path.Combine(baseDirectory, "tools", "ffmpeg", "ffmpeg.exe");
                yield return Path.Combine(baseDirectory, "tools", "ffmpeg", "bin", "ffmpeg.exe");
            }

            var assemblyDirectory = Path.GetDirectoryName(typeof(WindowsSenderSession).Assembly.Location);
            if (!string.IsNullOrWhiteSpace(assemblyDirectory))
            {
                yield return Path.Combine(assemblyDirectory, "ffmpeg.exe");
                yield return Path.Combine(assemblyDirectory, "ffmpeg", "ffmpeg.exe");
                yield return Path.Combine(assemblyDirectory, "ffmpeg", "bin", "ffmpeg.exe");
            }

            yield return "ffmpeg.exe";

            var path = Environment.GetEnvironmentVariable("PATH") ?? string.Empty;
            foreach (var part in path.Split(Path.PathSeparator, StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries))
            {
                yield return Path.Combine(part, "ffmpeg.exe");
            }

            var programFiles = Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles);
            var programFilesX86 = Environment.GetFolderPath(Environment.SpecialFolder.ProgramFilesX86);
            var localAppData = Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData);
            var userProfile = Environment.GetFolderPath(Environment.SpecialFolder.UserProfile);

            yield return Path.Combine(programFiles, "ffmpeg", "bin", "ffmpeg.exe");
            yield return Path.Combine(programFilesX86, "ffmpeg", "bin", "ffmpeg.exe");
            yield return Path.Combine(programFiles, "Topaz Labs LLC", "Topaz Video AI", "ffmpeg.exe");
            yield return Path.Combine(programFiles, "Steinberg", "Cubase 13", "Externals", "FFmpeg", "5.1.1", "ffmpeg.exe");
            yield return Path.Combine(programFilesX86, "Digiarty", "VideoProc Converter AI", "ffmpeg.exe");
            yield return Path.Combine(localAppData, "Microsoft", "WinGet", "Packages", "Gyan.FFmpeg*", "ffmpeg-*", "bin", "ffmpeg.exe");
            yield return Path.Combine(userProfile, "scoop", "apps", "ffmpeg", "current", "bin", "ffmpeg.exe");
            yield return Path.Combine("C:\\", "ffmpeg", "bin", "ffmpeg.exe");
        }

        foreach (var candidate in EnumerateCandidates().Distinct(StringComparer.OrdinalIgnoreCase))
        {
            if (string.IsNullOrWhiteSpace(candidate))
            {
                continue;
            }

            if (candidate.Contains('*'))
            {
                var directory = Path.GetDirectoryName(candidate);
                var fileName = Path.GetFileName(candidate);
                if (string.IsNullOrWhiteSpace(directory) || string.IsNullOrWhiteSpace(fileName))
                {
                    continue;
                }

                var root = directory;
                while (!string.IsNullOrEmpty(root) && root.Contains('*'))
                {
                    root = Path.GetDirectoryName(root);
                }

                if (string.IsNullOrWhiteSpace(root) || !Directory.Exists(root))
                {
                    continue;
                }

                var relative = directory[root.Length..].TrimStart(Path.DirectorySeparatorChar);
                var segments = relative.Split(Path.DirectorySeparatorChar, StringSplitOptions.RemoveEmptyEntries);
                var currentDirs = new List<string> { root };
                foreach (var segment in segments)
                {
                    var nextDirs = new List<string>();
                    foreach (var current in currentDirs)
                    {
                        nextDirs.AddRange(Directory.EnumerateDirectories(current, segment));
                    }

                    currentDirs = nextDirs;
                    if (currentDirs.Count == 0)
                    {
                        break;
                    }
                }

                foreach (var current in currentDirs)
                {
                    var expanded = Path.Combine(current, fileName);
                    if (File.Exists(expanded))
                    {
                        ffmpegPath = expanded;
                        return true;
                    }
                }

                continue;
            }

            if (File.Exists(candidate))
            {
                ffmpegPath = candidate;
                return true;
            }
        }

        ffmpegPath = string.Empty;
        return false;
    }

    private static void TryTerminateProcess(Process process)
    {
        try
        {
            if (!process.HasExited)
            {
                process.Kill(entireProcessTree: true);
            }
        }
        catch
        {
        }
    }

    private static void TryRaiseProcessPriority(Process process, ProcessPriorityClass priorityClass)
    {
        try
        {
            process.PriorityClass = priorityClass;
        }
        catch
        {
        }
    }

    private static void TryRaiseCurrentThreadPriority(ThreadPriority priority)
    {
        try
        {
            Thread.CurrentThread.Priority = priority;
        }
        catch
        {
        }
    }

    private static WindowsCaptureTargetInfo ResolveCaptureTarget(string deviceName)
    {
        var target = GetCaptureTargets().FirstOrDefault(screen => string.Equals(screen.DeviceName, deviceName, StringComparison.OrdinalIgnoreCase));
        return target ?? GetCaptureTargets().FirstOrDefault() ?? throw new InvalidOperationException("No display targets available");
    }

    private static bool IsDesktopDuplicationUnsupported(Exception ex)
    {
        for (var current = ex; current is not null; current = current.InnerException!)
        {
            if ((uint)current.HResult == 0x887A0004U ||
                current.Message.Contains("DXGI_ERROR_UNSUPPORTED", StringComparison.OrdinalIgnoreCase))
            {
                return true;
            }
        }

        return false;
    }

    private static Size ScaleToFit(Size source, Size maxSize)
    {
        var scale = Math.Min(maxSize.Width / (double)Math.Max(1, source.Width), maxSize.Height / (double)Math.Max(1, source.Height));
        var width = Math.Max(2, ((int)Math.Round(source.Width * scale)) & ~1);
        var height = Math.Max(2, ((int)Math.Round(source.Height * scale)) & ~1);
        return new Size(width, height);
    }

    private static byte[] NormalizeToAnnexB(byte[] payload)
    {
        if (payload.Length >= 4 &&
            payload[0] == 0 &&
            payload[1] == 0 &&
            ((payload[2] == 0 && payload[3] == 1) || payload[2] == 1))
        {
            return payload;
        }

        using var stream = new MemoryStream(payload.Length + 128);
        var offset = 0;
        while (offset + 4 <= payload.Length)
        {
            var nalLength = (payload[offset] << 24) | (payload[offset + 1] << 16) | (payload[offset + 2] << 8) | payload[offset + 3];
            offset += 4;
            if (nalLength <= 0 || offset + nalLength > payload.Length)
            {
                return payload;
            }

            stream.Write(new byte[] { 0, 0, 0, 1 });
            stream.Write(payload, offset, nalLength);
            offset += nalLength;
        }

        return stream.Length > 0 ? stream.ToArray() : payload;
    }

    private static byte[] ExtractCodecConfig(byte[] annexB, WindowsVideoCodec codec)
    {
        byte[]? vps = null;
        byte[]? sps = null;
        byte[]? pps = null;

        foreach (var nal in EnumerateAnnexBNals(annexB, codec))
        {
            switch (nal.Type)
            {
                case 32 when codec.IsHevc():
                    vps ??= nal.Data;
                    break;
                case 33 when codec.IsHevc():
                case 7 when !codec.IsHevc():
                    sps ??= nal.Data;
                    break;
                case 34 when codec.IsHevc():
                case 8 when !codec.IsHevc():
                    pps ??= nal.Data;
                    break;
            }
        }

        using var stream = new MemoryStream();
        if (codec.IsHevc() && vps is not null)
        {
            stream.Write(vps, 0, vps.Length);
        }

        if (sps is not null)
        {
            stream.Write(sps, 0, sps.Length);
        }

        if (pps is not null)
        {
            stream.Write(pps, 0, pps.Length);
        }

        return stream.ToArray();
    }

    private static byte[] StripCodecConfigNalUnits(byte[] annexB, WindowsVideoCodec codec)
    {
        using var stream = new MemoryStream();
        foreach (var nal in EnumerateAnnexBNals(annexB, codec))
        {
            if (IsCodecConfigNal(codec, nal.Type))
            {
                continue;
            }

            stream.Write(nal.Data, 0, nal.Data.Length);
        }

        return stream.ToArray();
    }

    private static bool ContainsKeyFrame(byte[] annexB, WindowsVideoCodec codec) =>
        EnumerateAnnexBNals(annexB, codec).Any(nal => IsKeyFrameNal(codec, nal.Type));

    private static IEnumerable<(byte Type, byte[] Data)> EnumerateAnnexBNals(byte[] annexB, WindowsVideoCodec codec)
    {
        var offset = 0;
        while (TryFindStartCode(annexB, offset, out var startCodeIndex, out var startCodeLength))
        {
            var nalStart = startCodeIndex + startCodeLength;
            if (!TryFindStartCode(annexB, nalStart, out var nextStartCodeIndex, out _))
            {
                nextStartCodeIndex = annexB.Length;
            }

            var nalLength = nextStartCodeIndex - startCodeIndex;
            if (nalLength > startCodeLength && nalStart < annexB.Length)
            {
                var data = new byte[nalLength];
                Buffer.BlockCopy(annexB, startCodeIndex, data, 0, nalLength);
                var nalType = GetNalType(codec, annexB[nalStart]);
                yield return (nalType, data);
            }

            offset = nextStartCodeIndex;
        }
    }

    private static bool TryFindStartCode(byte[] data, int offset, out int index, out int length)
    {
        for (var i = offset; i <= data.Length - 3; i++)
        {
            if (data[i] == 0 && data[i + 1] == 0)
            {
                if (data[i + 2] == 1)
                {
                    index = i;
                    length = 3;
                    return true;
                }

                if (i <= data.Length - 4 && data[i + 2] == 0 && data[i + 3] == 1)
                {
                    index = i;
                    length = 4;
                    return true;
                }
            }
        }

        index = -1;
        length = 0;
        return false;
    }

    private static byte GetNalType(WindowsVideoCodec codec, byte firstHeaderByte) =>
        codec.IsHevc()
            ? (byte)((firstHeaderByte >> 1) & 0x3F)
            : (byte)(firstHeaderByte & 0x1F);

    private static bool IsCodecConfigNal(WindowsVideoCodec codec, byte nalType) =>
        codec.IsHevc()
            ? nalType is 32 or 33 or 34
            : nalType is 7 or 8;

    private static bool IsKeyFrameNal(WindowsVideoCodec codec, byte nalType) =>
        codec.IsHevc()
            ? nalType is >= 16 and <= 21
            : nalType == 5;

    private IEncodedAccessUnitReader CreateEncodedAccessUnitReader() =>
        _selectedCodec.IsAv1()
            ? new IvfAccessUnitReader()
            : new AnnexBAccessUnitReader(_selectedCodec);

    private interface IEncodedAccessUnitReader
    {
        void Feed(ReadOnlySpan<byte> data, Action<byte[], bool> onAccessUnit);
        void Complete(Action<byte[], bool> onAccessUnit);
    }

    private sealed class AnnexBAccessUnitReader : IEncodedAccessUnitReader
    {
        private static readonly byte[] StartCode = { 0x00, 0x00, 0x00, 0x01 };

        private readonly WindowsVideoCodec _codec;
        private readonly List<byte> _buffer = new();
        private readonly List<byte[]> _currentAccessUnit = new();
        private byte[]? _latestVps;
        private byte[]? _latestSps;
        private byte[]? _latestPps;
        private bool _currentHasVcl;
        private bool _currentIsKeyFrame;
        private bool _currentHasVps;
        private bool _currentHasSps;
        private bool _currentHasPps;

        public AnnexBAccessUnitReader(WindowsVideoCodec codec)
        {
            _codec = codec;
        }

        public void Feed(ReadOnlySpan<byte> data, Action<byte[], bool> onAccessUnit)
        {
            for (var index = 0; index < data.Length; index++)
            {
                _buffer.Add(data[index]);
            }

            ParseAvailable(flushTail: false, onAccessUnit);
        }

        public void Complete(Action<byte[], bool> onAccessUnit)
        {
            ParseAvailable(flushTail: true, onAccessUnit);
            FlushCurrent(onAccessUnit);
            _buffer.Clear();
        }

        private void ParseAvailable(bool flushTail, Action<byte[], bool> onAccessUnit)
        {
            while (true)
            {
                var firstStartCodeIndex = FindStartCode(_buffer, 0);
                if (firstStartCodeIndex < 0)
                {
                    if (!flushTail && _buffer.Count > 4)
                    {
                        _buffer.RemoveRange(0, _buffer.Count - 4);
                    }
                    return;
                }

                if (firstStartCodeIndex > 0)
                {
                    _buffer.RemoveRange(0, firstStartCodeIndex);
                }

                var firstStartCodeLength = GetStartCodeLength(_buffer, 0);
                if (firstStartCodeLength == 0)
                {
                    return;
                }

                var nextStartCodeIndex = FindStartCode(_buffer, firstStartCodeLength);
                if (nextStartCodeIndex < 0)
                {
                    if (!flushTail)
                    {
                        return;
                    }

                    ProcessNal(_buffer.GetRange(firstStartCodeLength, _buffer.Count - firstStartCodeLength).ToArray(), onAccessUnit);
                    _buffer.Clear();
                    return;
                }

                ProcessNal(_buffer.GetRange(firstStartCodeLength, nextStartCodeIndex - firstStartCodeLength).ToArray(), onAccessUnit);
                _buffer.RemoveRange(0, nextStartCodeIndex);
            }
        }

        private void ProcessNal(byte[] nalUnit, Action<byte[], bool> onAccessUnit)
        {
            if (nalUnit.Length == 0)
            {
                return;
            }

            var nalType = GetNalType(_codec, nalUnit[0]);
            var isVcl = IsVclNal(nalType);
            var beginsNewPicture = isVcl && _currentHasVcl && IsFirstSliceOfPicture(_codec, nalUnit);

            if (IsAccessUnitDelimiter(nalType) || beginsNewPicture || (!isVcl && _currentHasVcl))
            {
                FlushCurrent(onAccessUnit);
            }

            if (IsAccessUnitDelimiter(nalType))
            {
                return;
            }

            _currentAccessUnit.Add(nalUnit);

            switch (nalType)
            {
                case 32 when _codec.IsHevc():
                    _latestVps = nalUnit.ToArray();
                    _currentHasVps = true;
                    break;
                case 33 when _codec.IsHevc():
                case 7 when !_codec.IsHevc():
                    _latestSps = nalUnit.ToArray();
                    _currentHasSps = true;
                    break;
                case 34 when _codec.IsHevc():
                case 8 when !_codec.IsHevc():
                    _latestPps = nalUnit.ToArray();
                    _currentHasPps = true;
                    break;
                default:
                    if (!isVcl)
                    {
                        break;
                    }

                    _currentHasVcl = true;
                    if (IsKeyFrameNal(_codec, nalType))
                    {
                        _currentIsKeyFrame = true;
                    }
                    break;
            }
        }

        private void FlushCurrent(Action<byte[], bool> onAccessUnit)
        {
            if (!_currentHasVcl || _currentAccessUnit.Count == 0)
            {
                ResetCurrent();
                return;
            }

            var nalUnits = new List<byte[]>();
            if (_currentIsKeyFrame)
            {
                if (_codec.IsHevc() && !_currentHasVps && _latestVps is not null)
                {
                    nalUnits.Add(_latestVps);
                }

                if (!_currentHasSps && _latestSps is not null)
                {
                    nalUnits.Add(_latestSps);
                }

                if (!_currentHasPps && _latestPps is not null)
                {
                    nalUnits.Add(_latestPps);
                }
            }

            nalUnits.AddRange(_currentAccessUnit);

            var totalBytes = nalUnits.Sum(static nal => StartCode.Length + nal.Length);
            var combined = GC.AllocateUninitializedArray<byte>(totalBytes);
            var offset = 0;
            foreach (var nalUnit in nalUnits)
            {
                Buffer.BlockCopy(StartCode, 0, combined, offset, StartCode.Length);
                offset += StartCode.Length;
                Buffer.BlockCopy(nalUnit, 0, combined, offset, nalUnit.Length);
                offset += nalUnit.Length;
            }

            onAccessUnit(combined, _currentIsKeyFrame);
            ResetCurrent();
        }

        private void ResetCurrent()
        {
            _currentAccessUnit.Clear();
            _currentHasVcl = false;
            _currentIsKeyFrame = false;
            _currentHasVps = false;
            _currentHasSps = false;
            _currentHasPps = false;
        }

        private static int FindStartCode(List<byte> buffer, int startIndex)
        {
            for (var index = Math.Max(0, startIndex); index <= buffer.Count - 3; index++)
            {
                if (buffer[index] == 0x00 && buffer[index + 1] == 0x00)
                {
                    if (buffer[index + 2] == 0x01)
                    {
                        return index;
                    }

                    if (index + 3 < buffer.Count && buffer[index + 2] == 0x00 && buffer[index + 3] == 0x01)
                    {
                        return index;
                    }
                }
            }

            return -1;
        }

        private static int GetStartCodeLength(List<byte> buffer, int index)
        {
            if (index + 2 < buffer.Count &&
                buffer[index] == 0x00 &&
                buffer[index + 1] == 0x00 &&
                buffer[index + 2] == 0x01)
            {
                return 3;
            }

            if (index + 3 < buffer.Count &&
                buffer[index] == 0x00 &&
                buffer[index + 1] == 0x00 &&
                buffer[index + 2] == 0x00 &&
                buffer[index + 3] == 0x01)
            {
                return 4;
            }

            return 0;
        }

        private static bool IsFirstSliceOfPicture(WindowsVideoCodec codec, byte[] nalUnit)
        {
            try
            {
                if (codec.IsHevc())
                {
                    if (nalUnit.Length <= 2)
                    {
                        return false;
                    }

                    var hevcRbsp = RemoveEmulationPreventionBytes(nalUnit, 2);
                    return hevcRbsp.Length > 0 && (hevcRbsp[0] & 0x80) != 0;
                }

                if (nalUnit.Length <= 1)
                {
                    return false;
                }

                var rbsp = RemoveEmulationPreventionBytes(nalUnit, 1);
                var bitReader = new H264BitReader(rbsp);
                return bitReader.ReadUnsignedExpGolomb() == 0;
            }
            catch
            {
                return false;
            }
        }

        private static byte[] RemoveEmulationPreventionBytes(byte[] data, int offset)
        {
            var buffer = new List<byte>(data.Length);
            var zeroCount = 0;
            for (var index = offset; index < data.Length; index++)
            {
                var value = data[index];
                if (zeroCount >= 2 && value == 0x03)
                {
                    zeroCount = 0;
                    continue;
                }

                buffer.Add(value);
                zeroCount = value == 0x00 ? zeroCount + 1 : 0;
            }

            return buffer.ToArray();
        }

        private bool IsAccessUnitDelimiter(byte nalType) => _codec.IsHevc() ? nalType == 35 : nalType == 9;

        private bool IsVclNal(byte nalType) => _codec.IsHevc() ? nalType <= 31 : nalType is 1 or 5;
    }

    private sealed class IvfAccessUnitReader : IEncodedAccessUnitReader
    {
        private readonly List<byte> _buffer = new();
        private bool _headerParsed;

        public void Feed(ReadOnlySpan<byte> data, Action<byte[], bool> onAccessUnit)
        {
            for (var index = 0; index < data.Length; index++)
            {
                _buffer.Add(data[index]);
            }

            ParseAvailable(onAccessUnit);
        }

        public void Complete(Action<byte[], bool> onAccessUnit)
        {
            ParseAvailable(onAccessUnit);
            _buffer.Clear();
        }

        private void ParseAvailable(Action<byte[], bool> onAccessUnit)
        {
            if (!_headerParsed)
            {
                if (_buffer.Count < 32)
                {
                    return;
                }

                _buffer.RemoveRange(0, 32);
                _headerParsed = true;
            }

            while (_buffer.Count >= 12)
            {
                var size = _buffer[0] |
                           (_buffer[1] << 8) |
                           (_buffer[2] << 16) |
                           (_buffer[3] << 24);
                if (size <= 0 || _buffer.Count < 12 + size)
                {
                    return;
                }

                var frame = _buffer.Skip(12).Take(size).ToArray();
                _buffer.RemoveRange(0, 12 + size);
                onAccessUnit(frame, true);
            }
        }
    }

    private sealed class H264BitReader
    {
        private readonly byte[] _buffer;
        private int _bitOffset;

        public H264BitReader(byte[] buffer)
        {
            _buffer = buffer;
        }

        public int ReadBit()
        {
            if (_bitOffset >= _buffer.Length * 8)
            {
                return 0;
            }

            var byteIndex = _bitOffset / 8;
            var shift = 7 - (_bitOffset % 8);
            _bitOffset++;
            return (_buffer[byteIndex] >> shift) & 0x01;
        }

        public int ReadUnsignedExpGolomb()
        {
            var leadingZeros = 0;
            while (ReadBit() == 0 && leadingZeros < 31)
            {
                leadingZeros++;
            }

            var codeNum = 1;
            for (var index = 0; index < leadingZeros; index++)
            {
                codeNum = (codeNum << 1) | ReadBit();
            }

            return codeNum - 1;
        }
    }

    private sealed record SenderCaptureContext(
        IDXGIAdapter1 Adapter,
        ID3D11Device Device,
        ID3D11DeviceContext Context,
        IDXGIOutputDuplication Duplication,
        ID3D11Texture2D StagingTexture,
        int SourceWidth,
        int SourceHeight) : IDisposable
    {
        public void Dispose()
        {
            StagingTexture.Dispose();
            Duplication.Dispose();
            Context.Dispose();
            Device.Dispose();
            Adapter.Dispose();
        }
    }

    private sealed record GdiCaptureContext(
        Rectangle Bounds,
        Bitmap Bitmap,
        Graphics Graphics) : IDisposable
    {
        public int SourceWidth => Bounds.Width;
        public int SourceHeight => Bounds.Height;

        public void Dispose()
        {
            Graphics.Dispose();
            Bitmap.Dispose();
        }
    }

    private sealed class SenderEncoderContext : IDisposable
    {
        private readonly SampleGrabberCallback _callback;
        private readonly IMFActivate _activate;
        private readonly IMFMediaSink _mediaSink;
        private readonly IMFAttributes _writerAttributes;
        private readonly IMFMediaType _outputType;
        private readonly IMFMediaType _inputType;
        private IMFSinkWriter _sinkWriter;
        private int _streamIndex;

        public SenderEncoderContext(
            SampleGrabberCallback callback,
            IMFActivate activate,
            IMFMediaSink mediaSink,
            IMFAttributes writerAttributes,
            IMFMediaType outputType,
            IMFMediaType inputType,
            IMFSinkWriter sinkWriter,
            int streamIndex)
        {
            _callback = callback;
            _activate = activate;
            _mediaSink = mediaSink;
            _writerAttributes = writerAttributes;
            _outputType = outputType;
            _inputType = inputType;
            _sinkWriter = sinkWriter;
            _streamIndex = streamIndex;
        }

        public void WriteFrame(byte[] bgraBytes, int stride, long sampleTimeHns)
        {
            using var sample = MediaFactory.MFCreateSample();
            using var buffer = MediaFactory.MFCreateMemoryBuffer(bgraBytes.Length);
            buffer.Lock(out var dataPointer, out _, out _);
            try
            {
                Marshal.Copy(bgraBytes, 0, dataPointer, bgraBytes.Length);
                buffer.CurrentLength = bgraBytes.Length;
            }
            finally
            {
                buffer.Unlock();
            }

            sample.AddBuffer(buffer);
            sample.SampleTime = sampleTimeHns;
            sample.SampleDuration = 0;
            sample.Set(MediaTypeAttributeKeys.DefaultStride, (uint)Math.Max(0, stride)).CheckError();
            _sinkWriter.WriteSample(_streamIndex, sample);
        }

        public void Reinitialize()
        {
            _sinkWriter.Finalize();
            _sinkWriter.Dispose();
            _callback.Reset();

            _sinkWriter = MediaFactory.MFCreateSinkWriterFromMediaSink(_mediaSink, _writerAttributes);
            using var streamSink = _mediaSink.GetStreamSinkByIndex(0);
            _streamIndex = streamSink.Identifier;
            _sinkWriter.SetInputMediaType(_streamIndex, _inputType, null);
            _sinkWriter.BeginWriting();
        }

        public void Dispose()
        {
            try
            {
                _sinkWriter.Finalize();
            }
            catch
            {
            }

            _sinkWriter.Dispose();
            _inputType.Dispose();
            _outputType.Dispose();
            _writerAttributes.Dispose();
            _mediaSink.Dispose();
            _activate.ShutdownObject();
            _activate.Dispose();
            _callback.Dispose();
        }
    }

    private sealed class SampleGrabberCallback : CallbackBase, IMFSampleGrabberSinkCallback
    {
        private readonly Action<long, byte[]> _onSample;

        public SampleGrabberCallback(Action<long, byte[]> onSample)
        {
            _onSample = onSample;
        }

        public void Reset()
        {
        }

        public void OnSetPresentationClock(IMFPresentationClock presentationClock)
        {
        }

        public void OnProcessSample(Guid majorMediaType, int sampleFlags, long sampleTime, long sampleDuration, Span<byte> sampleBuffer)
        {
            if (sampleBuffer.Length == 0)
            {
                return;
            }

            _onSample(sampleTime, sampleBuffer.ToArray());
        }

        public void OnShutdown()
        {
        }

        public void OnClockStart(long systemTime, long clockStartOffset)
        {
        }

        public void OnClockStop(long systemTime)
        {
        }

        public void OnClockPause(long systemTime)
        {
        }

        public void OnClockRestart(long systemTime)
        {
        }

        public void OnClockSetRate(long systemTime, float rate)
        {
        }
    }

    private sealed record EncoderPlan(string Name, Guid InputSubtype, bool HardwareTransforms);
    private sealed record SenderAudioConfig(int SampleRate, int Channels, int BytesPerSample);
    private sealed class SenderReconfigureRequestedException : OperationCanceledException;
}
