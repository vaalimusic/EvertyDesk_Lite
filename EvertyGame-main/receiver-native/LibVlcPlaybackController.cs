using LibVLCSharp.Shared;
using LibVLCSharp.WinForms;
using System.Windows.Forms;

namespace ReceiverNative;

internal sealed class LibVlcPlaybackController : IPlaybackController
{
    private static int _coreInitialized;
    private readonly Control _playbackHost;
    private readonly VideoView? _videoView;
    private readonly Panel? _nativeSurface;
    private readonly LibVLC _libVlc;
    private readonly MediaPlayer _mediaPlayer;
    private readonly bool _forceDirect3D11Vout;
    private readonly bool _directWindowBinding;
    private Media? _media;
    private LatestAccessUnitStream? _stream;
    private SessionConfig? _sessionConfig;
    private HardwareDecodeMode _hardwareDecodeMode = HardwareDecodeMode.Auto;
    private bool _aggressiveMode = true;
    private bool _ultraLowLatencyMode;
    private bool _disposed;
    private string _currentStatus = "Idle";
    private readonly System.Windows.Forms.Timer _statusRefreshTimer;

    public LibVlcPlaybackController(Control playbackHost, bool forceDirect3D11Vout, bool directWindowBinding = false)
    {
        EnsureCoreInitialized();

        _playbackHost = playbackHost;
        _forceDirect3D11Vout = forceDirect3D11Vout;
        _directWindowBinding = directWindowBinding;
        _playbackHost.Controls.Clear();
        if (_directWindowBinding)
        {
            _nativeSurface = new Panel
            {
                Dock = DockStyle.Fill,
                BackColor = Color.Black,
                Margin = Padding.Empty,
            };
            _playbackHost.Controls.Add(_nativeSurface);
        }
        else
        {
            _videoView = new VideoView
            {
                Dock = DockStyle.Fill,
                BackColor = Color.Black,
            };
            _playbackHost.Controls.Add(_videoView);
        }

        var libVlcOptions = new List<string>
        {
            "--no-audio",
            "--drop-late-frames",
            "--skip-frames",
            "--clock-jitter=0",
            "--clock-synchro=0",
            "--intf=dummy",
            "--quiet",
            "--no-video-title-show",
        };
        if (_forceDirect3D11Vout)
        {
            libVlcOptions.Add("--vout=direct3d11");
        }

        _libVlc = new LibVLC(libVlcOptions.ToArray());
        _mediaPlayer = new MediaPlayer(_libVlc);
        ApplyPlayerTuning();
        if (_directWindowBinding)
        {
            _nativeSurface!.HandleCreated += OnNativeSurfaceHandleCreated;
            _nativeSurface.HandleDestroyed += OnNativeSurfaceHandleDestroyed;
            if (_nativeSurface.IsHandleCreated)
            {
                AttachNativeSurface();
            }
        }
        else
        {
            _videoView!.MediaPlayer = _mediaPlayer;
        }

        _mediaPlayer.Opening += (_, _) => UpdateStatus("Opening");
        _mediaPlayer.Buffering += (_, args) => UpdateStatus($"Buffering {args.Cache:0}%");
        _mediaPlayer.Playing += (_, _) => UpdateStatus("Playing");
        _mediaPlayer.Stopped += (_, _) => UpdateStatus("Stopped");
        _mediaPlayer.EncounteredError += (_, _) => UpdateStatus("Playback error");

        _statusRefreshTimer = new System.Windows.Forms.Timer
        {
            Interval = 250,
            Enabled = true,
        };
        _statusRefreshTimer.Tick += (_, _) =>
        {
            if (_mediaPlayer.IsPlaying && _currentStatus.StartsWith("Buffering", StringComparison.OrdinalIgnoreCase))
            {
                UpdateStatus("Playing");
            }
        };
    }

    public event Action<string>? StatusChanged;
    public event Action<PlaybackStreamStats>? StreamStatsChanged;
    public event Action<PlaybackStreamStats>? EnhancementStreamStatsChanged;
    public event Action<long>? FrameDecoded;
    public event Action<long>? FramePresented;

    public string BackendLabel => (_directWindowBinding, _forceDirect3D11Vout) switch
    {
        (true, true) => PlaybackBackendKind.LibVlcHwndDirect3D11.ToUiLabel(),
        (false, true) => PlaybackBackendKind.LibVlcDirect3D11.ToUiLabel(),
        _ => PlaybackBackendKind.LibVlcDefault.ToUiLabel(),
    };
    public long LastPresentedBasePresentationTimeUs => 0;

    public void UpdateHardwareDecodeMode(HardwareDecodeMode mode)
    {
        if (_hardwareDecodeMode == mode)
        {
            return;
        }

        _hardwareDecodeMode = mode;
        ApplyPlayerTuning();
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
    }

    public void UpdatePacingWindow(TimeSpan minDelay, TimeSpan maxDelay)
    {
    }

    public void ApplySessionConfig(SessionConfig config)
    {
        var codecChanged = _sessionConfig is null ||
            !string.Equals(_sessionConfig.Codec, config.Codec, StringComparison.OrdinalIgnoreCase);
        var transportChanged = _sessionConfig is null ||
            !string.Equals(_sessionConfig.Transport, config.Transport, StringComparison.OrdinalIgnoreCase);

        _sessionConfig = config;
        if (_stream is null || codecChanged || transportChanged)
        {
            RestartPlayback();
        }
    }

    public void EnqueueAccessUnit(byte[] bytes, bool isKeyFrame, long presentationTimeUs)
    {
        _stream?.Enqueue(bytes, isKeyFrame);
    }

    public void EnqueueEnhancementAccessUnit(byte[] bytes, bool isKeyFrame, long presentationTimeUs, RoiMetadata? metadata)
    {
    }

    public void WaitForKeyFrame()
    {
        _stream?.WaitForKeyFrame();
    }

    public void PrepareForSessionStop()
    {
        _stream?.WaitForKeyFrame();
    }

    public void PrepareForKeyFrameRecovery()
    {
        _stream?.WaitForKeyFrame();
    }

    public void ResetEnhancementPath()
    {
    }

    public void Dispose()
    {
        if (_disposed)
        {
            return;
        }

        _disposed = true;
        _statusRefreshTimer.Stop();
        _statusRefreshTimer.Dispose();
        if (_directWindowBinding)
        {
            RunOnPlaybackHostThread(DetachAndRemoveNativeSurface);
        }
        _mediaPlayer.Stop();
        _media?.Dispose();
        _stream?.Dispose();
        _mediaPlayer.Dispose();
        _libVlc.Dispose();
        RunOnPlaybackHostThread(() =>
        {
            if (_videoView is not null)
            {
                if (_videoView.Parent == _playbackHost)
                {
                    _playbackHost.Controls.Remove(_videoView);
                }

                _videoView.Dispose();
            }
        });
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
        if (_sessionConfig is null)
        {
            return;
        }

        UpdateStatus("Restarting playback");
        if (_directWindowBinding)
        {
            AttachNativeSurface();
        }

        _mediaPlayer.Stop();
        _media?.Dispose();
        _stream?.Dispose();

        var isAdbTunnel = string.Equals(_sessionConfig.Transport, "EVRT_REALTIME_V2_TCP_ADB", StringComparison.OrdinalIgnoreCase);
        var lowLatencyDecodeProfile =
            _ultraLowLatencyMode ||
            _aggressiveMode ||
            _hardwareDecodeMode.IsNvidiaLowLatencyProfile() ||
            _hardwareDecodeMode.IsIntelQuickSync();
        _stream = new LatestAccessUnitStream(
            maxQueuedUnits: lowLatencyDecodeProfile ? 1 : 2,
            maxQueuedBytes: isAdbTunnel
                ? (_ultraLowLatencyMode ? 256 * 1024 : (lowLatencyDecodeProfile ? 384 * 1024 : 768 * 1024))
                : (_ultraLowLatencyMode ? 384 * 1024 : (lowLatencyDecodeProfile ? 512 * 1024 : 1024 * 1024)),
            statsChanged: stats => StreamStatsChanged?.Invoke(stats),
            hardResetOnKeyFrame: isAdbTunnel || lowLatencyDecodeProfile || _ultraLowLatencyMode,
            dropCurrentOnWaitForKeyFrame: lowLatencyDecodeProfile || _ultraLowLatencyMode);

        _media = new Media(_libVlc, new StreamMediaInput(_stream));
        foreach (var option in BuildMediaOptions(_sessionConfig))
        {
            _media.AddOption(option);
        }

        _mediaPlayer.Play(_media);
    }

    private void ApplyPlayerTuning()
    {
        _mediaPlayer.EnableKeyInput = false;
        _mediaPlayer.EnableMouseInput = false;
        _mediaPlayer.EnableHardwareDecoding = _hardwareDecodeMode != HardwareDecodeMode.Disabled;
        _mediaPlayer.NetworkCaching = 0;
        _mediaPlayer.FileCaching = 0;
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

    private IEnumerable<string> BuildMediaOptions(SessionConfig config)
    {
        var demux = config.Codec.Contains("av1", StringComparison.OrdinalIgnoreCase)
            ? "av1"
            : config.Codec.Contains("hevc", StringComparison.OrdinalIgnoreCase) ? "hevc" : "h264";
        var isAdbTunnel = string.Equals(config.Transport, "EVRT_REALTIME_V2_TCP_ADB", StringComparison.OrdinalIgnoreCase);
        var nvidiaLowLatency = _hardwareDecodeMode.IsNvidiaLowLatencyProfile();
        var intelQuickSync = _hardwareDecodeMode.IsIntelQuickSync();
        yield return $":demux={demux}";
        yield return ":no-audio";
        yield return ":codec=avcodec";
        yield return ":clock-jitter=0";
        yield return ":clock-synchro=0";
        yield return ":network-caching=0";
        yield return ":live-caching=0";
        yield return ":file-caching=0";
        yield return ":drop-late-frames";
        yield return ":skip-frames";
        yield return ":avcodec-fast";
        if (config.Fps > 0 && demux == "h264")
        {
            yield return $":h264-fps={config.Fps}";
        }
        if (isAdbTunnel)
        {
            yield return ":avcodec-skiploopfilter=all";
            yield return ":avcodec-threads=2";
        }
        if (_ultraLowLatencyMode)
        {
            yield return ":avcodec-direct-rendering";
            yield return ":avcodec-threads=1";
            yield return ":avcodec-skiploopfilter=all";
        }
        if (nvidiaLowLatency)
        {
            // Keep the already-working Windows D3D11 decode path, but bias VLC toward the leanest render pipeline.
            yield return ":avcodec-direct-rendering";
            yield return ":avcodec-skiploopfilter=all";
            yield return ":avcodec-skipidct=all";
            yield return ":avcodec-threads=1";
        }
        if (intelQuickSync)
        {
            // QSV is available in this LibVLC runtime; keep the queue small and avoid extra software work around it.
            yield return ":codec=qsv";
            yield return ":avcodec-threads=1";
            yield return ":avcodec-skiploopfilter=nonkey";
        }
        yield return $":avcodec-hw={_hardwareDecodeMode.ToVlcOption()}";
    }

    private static void EnsureCoreInitialized()
    {
        if (Interlocked.Exchange(ref _coreInitialized, 1) == 0)
        {
            Core.Initialize();
        }
    }

    private void OnNativeSurfaceHandleCreated(object? sender, EventArgs e)
    {
        AttachNativeSurface();
    }

    private void OnNativeSurfaceHandleDestroyed(object? sender, EventArgs e)
    {
        if (_disposed)
        {
            return;
        }

        _mediaPlayer.Hwnd = IntPtr.Zero;
    }

    private void AttachNativeSurface()
    {
        if (!_directWindowBinding || _nativeSurface is null || _nativeSurface.IsDisposed || !_nativeSurface.IsHandleCreated)
        {
            return;
        }

        _mediaPlayer.Hwnd = _nativeSurface.Handle;
    }

    private void DetachAndRemoveNativeSurface()
    {
        if (_nativeSurface is null)
        {
            return;
        }

        _nativeSurface.HandleCreated -= OnNativeSurfaceHandleCreated;
        _nativeSurface.HandleDestroyed -= OnNativeSurfaceHandleDestroyed;
        _mediaPlayer.Hwnd = IntPtr.Zero;
        if (_nativeSurface.Parent == _playbackHost)
        {
            _playbackHost.Controls.Remove(_nativeSurface);
        }
        _nativeSurface.Dispose();
    }

    private void RunOnPlaybackHostThread(Action action)
    {
        if (_playbackHost.IsDisposed)
        {
            return;
        }

        if (_playbackHost.IsHandleCreated && _playbackHost.InvokeRequired)
        {
            try
            {
                _playbackHost.Invoke(action);
            }
            catch (ObjectDisposedException)
            {
            }
            catch (InvalidOperationException)
            {
            }
            return;
        }

        action();
    }
}
