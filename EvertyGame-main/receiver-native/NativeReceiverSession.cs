using System.Diagnostics;
using System.Net;
using System.Net.Sockets;
using System.Windows.Forms;

namespace ReceiverNative;

internal sealed record ReceiverSessionSnapshot
{
    public bool Listening { get; init; }
    public string Status { get; init; } = "Idle";
    public string TransportMode { get; init; } = ReceiverTransportMode.Udp.ToUiLabel();
    public string PlaybackBackend { get; init; } = PlaybackBackendKind.MediaFoundationDirect3D11.ToUiLabel();
    public string PlaybackStatus { get; init; } = "-";
    public string DecodeMode { get; init; } = HardwareDecodeMode.Auto.ToUiLabel();
    public string StreamMode { get; init; } = "single";
    public string Codec { get; init; } = "-";
    public string Preset { get; init; } = "-";
    public string Resolution { get; init; } = "-";
    public int TargetFps { get; init; }
    public double BitrateMbps { get; init; }
    public long PacketsReceived { get; init; }
    public long SessionConfigPackets { get; init; }
    public long CodecConfigPackets { get; init; }
    public long VideoPackets { get; init; }
    public long AudioPackets { get; init; }
    public long ControlPackets { get; init; }
    public long FramesAssembled { get; init; }
    public long ReassemblyDroppedFrames { get; init; }
    public long EnhancementFramesAssembled { get; init; }
    public long EnhancementDroppedFrames { get; init; }
    public long StreamDroppedAccessUnits { get; init; }
    public int StreamQueuedAccessUnits { get; init; }
    public int StreamQueuedKilobytes { get; init; }
    public int EnhancementQueuedAccessUnits { get; init; }
    public int EnhancementQueuedKilobytes { get; init; }
    public bool WaitingForKeyFrame { get; init; }
    public int InputFpsProxy { get; init; }
    public int EnhancementFps { get; init; }
    public int AssemblyDelayMs { get; init; }
    public int ArrivalDeltaMs { get; init; } = -1;
    public int DecodeDeltaMs { get; init; } = -1;
    public int PresentDeltaMs { get; init; } = -1;
    public int PulseToPcEstimateMs { get; init; } = -1;
    public int TapToPcEstimateMs { get; init; } = -1;
    public int AdaptiveJitterMs { get; init; }
    public bool UltraLowLatencyMode { get; init; }
    public bool SystemHintsEnabled { get; init; }
    public string RemoteEndpoint { get; init; } = "-";
    public string LastPacketType { get; init; } = "-";
    public string LastPlaybackError { get; init; } = "-";
    public string LastBackendFailure { get; init; } = "-";
    public bool RoiActive { get; init; }
    public string RoiRect { get; init; } = "-";

    public long TotalDroppedFrames => ReassemblyDroppedFrames + StreamDroppedAccessUnits + EnhancementDroppedFrames;
}

internal sealed class NativeReceiverSession : IDisposable
{
    private readonly object _sync = new();
    private readonly object _playbackSync = new();
    private readonly Control _playbackHost;
    private IPlaybackController _playback;
    private IPlaybackController? _activePlaybackRef;
    private string _playbackBackendLabel = PlaybackBackendKind.MediaFoundationDirect3D11.ToUiLabel();
    private readonly SemaphoreSlim _controlSendGate = new(1, 1);
    private readonly WindowsPerformanceHints _performanceHints = new();
    private readonly AudioPlaybackSink _audioPlayback = new();
    private readonly AudioFrameReassembler _audioReassembler;

    private UdpClient? _udpClient;
    private TcpListener? _tcpListener;
    private TcpClient? _tcpClient;
    private NetworkStream? _tcpStream;
    private Process? _adbShellProcess;
    private FrameReassembler? _reassembler;
    private CancellationTokenSource? _cts;
    private Task? _receiveTask;
    private Task? _feedbackTask;
    private IPEndPoint? _remoteEndpoint;
    private RelayTransportRoute? _relayRoute;
    private RelayTransportRoute? _relayRegistrationRoute;
    private ReceiverSessionSnapshot _snapshot = new();
    private ReceiverTransportMode _transportMode = ReceiverTransportMode.Udp;
    private PlaybackBackendKind _playbackBackend = PlaybackBackendKind.MediaFoundationDirect3D11;
    private HardwareDecodeMode _decodeMode = HardwareDecodeMode.Auto;
    private bool _aggressiveMode = true;
    private bool _ultraLowLatencyMode = true;
    private int _listeningPort;
    private SessionConfig? _currentSessionConfig;
    private readonly Queue<long> _inputFrameTicks = new();
    private readonly Queue<long> _enhancementFrameTicks = new();
    private readonly Dictionary<long, long> _baseFrameArrivalTicksByPts = new();
    private readonly Dictionary<long, LatencyPulseControl> _pendingLatencyPulsesByPts = new();
    private readonly Queue<int> _recentPulseToPcEstimates = new();
    private readonly Queue<int> _recentTapToPcEstimates = new();
    private long _lastFeedbackDrops;
    private long _lastRequestKeyFrameAtTicks;
    private long _lastFeedbackAtTicks;
    private long _lastVideoArrivalAtTicks;
    private long _lastFrameDecodedAtTicks;
    private long _lastFramePresentedAtTicks;
    private long _lastPresentedBasePresentationTimeUs;
    private long _lastPresentedBaseTicks;
    private long _sessionReadyAtTicks;
    private long _lastForcedCatchUpAtTicks;
    private bool _videoStallLogged;
    private bool _decodeStallLogged;
    private bool _presentStallLogged;
    private int _adaptiveJitterMs;
    private int _manualAdaptiveJitterMs;
    private int _manualAudioBufferMs;
    private int _manualCatchUpThresholdMs;
    private int _manualKeyFrameCooldownMs;
    private int _manualPanicQueueAu;
    private int _manualFeedbackTickMs;
    private int _manualHighDeltaMs;
    private int _manualCriticalDeltaMs;
    private int _manualStartupGraceMs;
    private int _manualDropBurstStep;
    private TimeSpan _manualMinPacingDelay = TimeSpan.Zero;
    private TimeSpan _manualMaxPacingDelay = TimeSpan.Zero;
    private int _sessionGeneration;
    private long _lastRelayRegistrationAtTicks;

    public NativeReceiverSession(Control playbackHost)
    {
        _playbackHost = playbackHost;
        _playback = CreatePlaybackController(_playbackBackend);
        _activePlaybackRef = _playback;
        _playbackBackendLabel = _playback.BackendLabel;
        _audioReassembler = new AudioFrameReassembler(frame => _audioPlayback.EnqueuePcmFrame(frame));
    }

    public ReceiverSessionSnapshot GetSnapshot()
    {
        lock (_sync)
        {
            return _snapshot;
        }
    }

    public void ConfigureRelayRoute(RelayTransportRoute? route)
    {
        lock (_sync)
        {
            _relayRoute = route;
            _lastRelayRegistrationAtTicks = 0;
        }
    }

    public void ConfigureRelayRegistrationRoute(RelayTransportRoute? route)
    {
        lock (_sync)
        {
            _relayRegistrationRoute = route;
            _lastRelayRegistrationAtTicks = 0;
        }

        if (route is not null && _transportMode == ReceiverTransportMode.Udp && GetSnapshot().Listening)
        {
            SendRelayRegistrationFireAndForget();
        }
    }

    private void ExecutePlaybackMutation(string operationName, Action action, int timeoutMs = 500)
    {
        var entered = false;
        try
        {
            entered = Monitor.TryEnter(_playbackSync, timeoutMs);
            if (!entered)
            {
                ReceiverTrace.Log($"{operationName} skipped: playback lock timeout");
                throw new TimeoutException($"{operationName} failed: playback lock timeout");
            }

            action();
        }
        finally
        {
            if (entered)
            {
                Monitor.Exit(_playbackSync);
            }
        }
    }

    private bool TryExecutePlaybackMutation(string operationName, Action action, int timeoutMs = 250)
    {
        var entered = false;
        try
        {
            entered = Monitor.TryEnter(_playbackSync, timeoutMs);
            if (!entered)
            {
                ReceiverTrace.Log($"{operationName} skipped: playback lock timeout");
                return false;
            }

            action();
            return true;
        }
        catch (Exception ex)
        {
            ReceiverTrace.Log(ex, $"{operationName} failed");
            return false;
        }
        finally
        {
            if (entered)
            {
                Monitor.Exit(_playbackSync);
            }
        }
    }

    public void Start(int port, ReceiverTransportMode transportMode, HardwareDecodeMode decodeMode, bool aggressiveMode)
    {
        Stop();
        _performanceHints.Enable();
        var sessionGeneration = Interlocked.Increment(ref _sessionGeneration);

        _transportMode = transportMode;
        _decodeMode = decodeMode;
        _aggressiveMode = aggressiveMode;
        _listeningPort = port;
        ExecutePlaybackMutation("Session Start playback init", () =>
        {
            _playback.UpdateHardwareDecodeMode(decodeMode);
            _playback.UpdateAggressiveMode(GetEffectiveAggressiveMode());
            _playback.UpdateUltraLowLatencyMode(GetEffectiveUltraLowLatencyMode());
            _playback.UpdateAdaptiveJitterBuffer(GetAdaptiveJitterDelay());
            _playback.UpdatePacingWindow(_manualMinPacingDelay, _manualMaxPacingDelay);
        });
        _audioPlayback.UpdateManualBufferDurationMs(_manualAudioBufferMs);
        var playbackBackendLabel = GetPlaybackBackendLabel();

        UdpClient? udpClient = null;
        TcpListener? tcpListener = null;
        if (transportMode == ReceiverTransportMode.Udp)
        {
            udpClient = new UdpClient(port);
            udpClient.Client.ReceiveBufferSize = 512 * 1024;
            udpClient.Client.SendBufferSize = 64 * 1024;
            if (udpClient.Client.LocalEndPoint is IPEndPoint localUdpEndpoint)
            {
                ReceiverTrace.Log($"Session UDP bind opened on {localUdpEndpoint.Address}:{localUdpEndpoint.Port}");
            }
            else
            {
                ReceiverTrace.Log($"Session UDP bind opened on port {port}");
            }
        }
        else if (transportMode == ReceiverTransportMode.AdbTunnelTcp)
        {
            tcpListener = new TcpListener(IPAddress.Any, port);
            tcpListener.Server.ReceiveBufferSize = 512 * 1024;
            tcpListener.Server.SendBufferSize = 64 * 1024;
            tcpListener.Start(1);
            if (tcpListener.LocalEndpoint is IPEndPoint localTcpEndpoint)
            {
                ReceiverTrace.Log($"Session TCP listener opened on {localTcpEndpoint.Address}:{localTcpEndpoint.Port}");
            }
            else
            {
                ReceiverTrace.Log($"Session TCP listener opened on port {port}");
            }
        }

        var reassembler = new FrameReassembler(
            onSessionConfig: config => HandleSessionConfig(sessionGeneration, config),
            onBaseAccessUnitReady: (bytes, isKeyFrame, assemblyDelayMs, presentationTimeUs) =>
                HandleAccessUnitReady(sessionGeneration, bytes, isKeyFrame, assemblyDelayMs, presentationTimeUs),
            onEnhancementAccessUnitReady: accessUnit => HandleEnhancementAccessUnitReady(sessionGeneration, accessUnit),
            onDroppedFramesChanged: (baseDropped, enhancementDropped) =>
            {
                lock (_sync)
                {
                    _snapshot = _snapshot with
                    {
                        ReassemblyDroppedFrames = baseDropped,
                        EnhancementDroppedFrames = enhancementDropped,
                    };
                }
            });

        var cts = new CancellationTokenSource();
        _udpClient = udpClient;
        _tcpListener = tcpListener;
        _tcpClient = null;
        _tcpStream = null;
        _adbShellProcess = null;
        _reassembler = reassembler;
        _cts = cts;
        lock (_sync)
        {
            _snapshot = new ReceiverSessionSnapshot
            {
                Listening = true,
                Status = transportMode.BuildWaitingStatus(port),
                TransportMode = transportMode.ToUiLabel(),
                PlaybackBackend = playbackBackendLabel,
                PlaybackStatus = "Idle",
                DecodeMode = decodeMode.ToUiLabel(),
                UltraLowLatencyMode = GetEffectiveUltraLowLatencyMode(),
                AdaptiveJitterMs = GetEffectiveAdaptiveJitterMs(),
                SystemHintsEnabled = true,
                LastBackendFailure = "-",
                RemoteEndpoint = transportMode == ReceiverTransportMode.AdbShellH264 ? "adb exec-out screenrecord" : "-",
            };
            _remoteEndpoint = null;
            _inputFrameTicks.Clear();
            _enhancementFrameTicks.Clear();
            _lastFeedbackDrops = 0;
            _lastRequestKeyFrameAtTicks = 0;
            _lastFeedbackAtTicks = 0;
            _lastVideoArrivalAtTicks = 0;
            _lastFrameDecodedAtTicks = 0;
            _lastFramePresentedAtTicks = 0;
            _lastPresentedBasePresentationTimeUs = 0;
            _lastPresentedBaseTicks = 0;
            _sessionReadyAtTicks = 0;
            _lastForcedCatchUpAtTicks = 0;
            _videoStallLogged = false;
            _decodeStallLogged = false;
            _presentStallLogged = false;
            _currentSessionConfig = null;
            _baseFrameArrivalTicksByPts.Clear();
            _pendingLatencyPulsesByPts.Clear();
            _recentPulseToPcEstimates.Clear();
            _recentTapToPcEstimates.Clear();
            _lastRelayRegistrationAtTicks = 0;
        }

        _receiveTask = transportMode switch
        {
            ReceiverTransportMode.Udp => Task.Run(() => ReceiveUdpLoopAsync(cts.Token)),
            ReceiverTransportMode.AdbTunnelTcp => Task.Run(() => ReceiveTcpLoopAsync(cts.Token)),
            ReceiverTransportMode.AdbShellH264 => Task.Run(() => ReceiveAdbShellCaptureLoopAsync(sessionGeneration, cts.Token)),
            _ => Task.Run(() => ReceiveUdpLoopAsync(cts.Token)),
        };
        _feedbackTask = transportMode == ReceiverTransportMode.AdbShellH264
            ? null
            : Task.Run(() => FeedbackLoopAsync(cts.Token));
    }

    public void UpdateHardwareDecodeMode(HardwareDecodeMode mode)
    {
        if (_decodeMode == mode)
        {
            return;
        }

        _decodeMode = mode;
        ExecutePlaybackMutation("UpdateHardwareDecodeMode", () => _playback.UpdateHardwareDecodeMode(mode));
        lock (_sync)
        {
            _snapshot = _snapshot with { DecodeMode = mode.ToUiLabel() };
        }
        RequestKeyFrame();
    }

    public void UpdatePlaybackBackend(PlaybackBackendKind backend)
    {
        if (_playbackBackend == backend)
        {
            return;
        }

        IPlaybackController? previousPlayback = null;
        IPlaybackController? newPlayback = null;
        SessionConfig? sessionConfig = null;
        var listening = false;

        ExecutePlaybackMutation("UpdatePlaybackBackend", () =>
        {
            if (_playbackBackend == backend)
            {
                return;
            }

            previousPlayback = _playback;
            newPlayback = RunOnPlaybackHostThread(() => CreatePlaybackController(backend));
            newPlayback.UpdateHardwareDecodeMode(_decodeMode);
            newPlayback.UpdateAggressiveMode(GetEffectiveAggressiveMode());
            newPlayback.UpdateUltraLowLatencyMode(GetEffectiveUltraLowLatencyMode());
            newPlayback.UpdateAdaptiveJitterBuffer(GetAdaptiveJitterDelay());
            newPlayback.UpdatePacingWindow(_manualMinPacingDelay, _manualMaxPacingDelay);

            sessionConfig = _currentSessionConfig;
            if (sessionConfig is not null)
            {
                newPlayback.ApplySessionConfig(sessionConfig);
                newPlayback.WaitForKeyFrame();
            }

            _playback = newPlayback;
            _activePlaybackRef = newPlayback;
            _playbackBackend = backend;
            _playbackBackendLabel = newPlayback.BackendLabel;
            listening = GetSnapshot().Listening;
        });

        if (newPlayback is null)
        {
            return;
        }

        previousPlayback?.Dispose();

        lock (_sync)
        {
            _lastPresentedBasePresentationTimeUs = 0;
            _lastPresentedBaseTicks = 0;
            _baseFrameArrivalTicksByPts.Clear();
            _pendingLatencyPulsesByPts.Clear();
            _snapshot = _snapshot with
            {
                PlaybackBackend = newPlayback.BackendLabel,
                PlaybackStatus = listening ? "Switching backend" : "Idle",
                LastPlaybackError = "-",
                LastBackendFailure = "-",
                AdaptiveJitterMs = _adaptiveJitterMs,
                PulseToPcEstimateMs = -1,
                TapToPcEstimateMs = -1,
            };
        }

        if (listening)
        {
            RequestKeyFrame();
        }
    }

    public void UpdateAggressiveMode(bool enabled)
    {
        ReceiverTrace.Log($"Session UpdateAggressiveMode begin: {_aggressiveMode} -> {enabled}");
        if (_aggressiveMode == enabled)
        {
            ReceiverTrace.Log("Session UpdateAggressiveMode skipped: no effective change");
            return;
        }

        _aggressiveMode = enabled;
        ExecutePlaybackMutation("UpdateAggressiveMode", () => _playback.UpdateAggressiveMode(GetEffectiveAggressiveMode()));
        ReceiverTrace.Log("Session UpdateAggressiveMode end");
        RequestKeyFrame();
    }

    public void UpdateUltraLowLatencyMode(bool enabled)
    {
        ReceiverTrace.Log($"Session UpdateUltraLowLatencyMode begin: {_ultraLowLatencyMode} -> {enabled}");
        if (_ultraLowLatencyMode == enabled)
        {
            lock (_sync)
            {
                _snapshot = _snapshot with
                {
                    UltraLowLatencyMode = GetEffectiveUltraLowLatencyMode(),
                    AdaptiveJitterMs = enabled && _transportMode == ReceiverTransportMode.Udp ? GetEffectiveAdaptiveJitterMs() : 0,
                };
            }
            ReceiverTrace.Log("Session UpdateUltraLowLatencyMode skipped: no effective change");
            return;
        }

        _ultraLowLatencyMode = enabled;
        ExecutePlaybackMutation("UpdateUltraLowLatencyMode", () =>
        {
            _playback.UpdateUltraLowLatencyMode(GetEffectiveUltraLowLatencyMode());
            _playback.UpdateAdaptiveJitterBuffer(GetAdaptiveJitterDelay());
        });
        lock (_sync)
        {
            _snapshot = _snapshot with
            {
                UltraLowLatencyMode = GetEffectiveUltraLowLatencyMode(),
                AdaptiveJitterMs = enabled && _transportMode == ReceiverTransportMode.Udp ? GetEffectiveAdaptiveJitterMs() : 0,
            };
        }
        ReceiverTrace.Log("Session UpdateUltraLowLatencyMode end");
        RequestKeyFrame();
    }

    public void UpdateAdaptiveJitterOverride(int valueMs)
    {
        _manualAdaptiveJitterMs = Math.Clamp(valueMs, 0, 80);
        ExecutePlaybackMutation("UpdateAdaptiveJitterOverride", () => _playback.UpdateAdaptiveJitterBuffer(GetAdaptiveJitterDelay()));
        lock (_sync)
        {
            _snapshot = _snapshot with
            {
                AdaptiveJitterMs = _transportMode == ReceiverTransportMode.Udp ? GetEffectiveAdaptiveJitterMs() : 0,
            };
        }
    }

    public void UpdateAudioBufferMs(int valueMs)
    {
        _manualAudioBufferMs = Math.Clamp(valueMs, 0, 1500);
        _audioPlayback.UpdateManualBufferDurationMs(_manualAudioBufferMs);
    }

    public void UpdateCatchUpThresholdMs(int valueMs)
    {
        _manualCatchUpThresholdMs = Math.Clamp(valueMs, 0, 120);
    }

    public void UpdateKeyFrameCooldownMs(int valueMs)
    {
        _manualKeyFrameCooldownMs = Math.Clamp(valueMs, 0, 2000);
    }

    public void UpdatePanicQueueThresholdAu(int valueAu)
    {
        _manualPanicQueueAu = Math.Clamp(valueAu, 0, 12);
    }

    public void UpdateFeedbackTickMs(int valueMs)
    {
        _manualFeedbackTickMs = Math.Clamp(valueMs, 0, 500);
    }

    public void UpdateHighDeltaThresholdMs(int valueMs)
    {
        _manualHighDeltaMs = Math.Clamp(valueMs, 0, 120);
    }

    public void UpdateCriticalDeltaThresholdMs(int valueMs)
    {
        _manualCriticalDeltaMs = Math.Clamp(valueMs, 0, 180);
    }

    public void UpdateStartupGraceMs(int valueMs)
    {
        _manualStartupGraceMs = Math.Clamp(valueMs, 0, 4000);
    }

    public void UpdateDropBurstStep(int value)
    {
        _manualDropBurstStep = Math.Clamp(value, 0, 20);
    }

    public void UpdatePacingWindowMs(int minMs, int maxMs)
    {
        _manualMinPacingDelay = minMs > 0 ? TimeSpan.FromMilliseconds(Math.Clamp(minMs, 0, 50)) : TimeSpan.Zero;
        _manualMaxPacingDelay = maxMs > 0 ? TimeSpan.FromMilliseconds(Math.Clamp(maxMs, 0, 50)) : TimeSpan.Zero;
        ExecutePlaybackMutation("UpdatePacingWindow", () => _playback.UpdatePacingWindow(_manualMinPacingDelay, _manualMaxPacingDelay));
    }

    public void ResetManualTuningOverrides()
    {
        _manualAdaptiveJitterMs = 0;
        _manualAudioBufferMs = 0;
        _manualCatchUpThresholdMs = 0;
        _manualKeyFrameCooldownMs = 0;
        _manualPanicQueueAu = 0;
        _manualFeedbackTickMs = 0;
        _manualHighDeltaMs = 0;
        _manualCriticalDeltaMs = 0;
        _manualStartupGraceMs = 0;
        _manualDropBurstStep = 0;
        _manualMinPacingDelay = TimeSpan.Zero;
        _manualMaxPacingDelay = TimeSpan.Zero;

        ExecutePlaybackMutation("ResetManualTuningOverrides", () =>
        {
            _playback.UpdateAdaptiveJitterBuffer(GetAdaptiveJitterDelay());
            _playback.UpdatePacingWindow(_manualMinPacingDelay, _manualMaxPacingDelay);
        });

        _audioPlayback.UpdateManualBufferDurationMs(0);

        lock (_sync)
        {
            _snapshot = _snapshot with
            {
                AdaptiveJitterMs = _transportMode == ReceiverTransportMode.Udp ? GetEffectiveAdaptiveJitterMs() : 0,
            };
        }
    }

    public void SendRemoteMouseMoveAbsolute(long seq, double x, double y)
    {
        SendControlPacketFireAndForget(ControlPacketBuilder.BuildRemoteMouseMoveAbsolute(seq, x, y));
    }

    public void SendRemoteMouseMoveRelative(long seq, int dx, int dy)
    {
        SendControlPacketFireAndForget(ControlPacketBuilder.BuildRemoteMouseMoveRelative(seq, dx, dy));
    }

    public void SendRemoteMouseButton(long seq, RemoteMouseButtonKind button, bool pressed)
    {
        SendControlPacketFireAndForget(ControlPacketBuilder.BuildRemoteMouseButton(seq, button, pressed));
    }

    public void SendRemoteMouseWheel(long seq, int delta)
    {
        SendControlPacketFireAndForget(ControlPacketBuilder.BuildRemoteMouseWheel(seq, delta));
    }

    public void SendRemoteKey(long seq, int virtualKey, bool pressed)
    {
        SendControlPacketFireAndForget(ControlPacketBuilder.BuildRemoteKey(seq, virtualKey, pressed));
    }

    public void SendRemoteReleaseAll(long seq)
    {
        SendControlPacketFireAndForget(ControlPacketBuilder.BuildRemoteReleaseAll(seq));
    }

    public void Stop()
    {
        ReceiverTrace.Log("Session Stop begin");
        Interlocked.Increment(ref _sessionGeneration);
        var cts = _cts;
        var udpClient = _udpClient;
        var tcpStream = _tcpStream;
        var tcpClient = _tcpClient;
        var tcpListener = _tcpListener;
        var adbShellProcess = _adbShellProcess;
        var receiveTask = _receiveTask;
        var feedbackTask = _feedbackTask;

        _cts = null;
        _udpClient = null;
        _tcpStream = null;
        _tcpClient = null;
        _tcpListener = null;
        _adbShellProcess = null;
        _reassembler = null;
        _receiveTask = null;
        _feedbackTask = null;
        _relayRoute = null;
        _relayRegistrationRoute = null;
        _lastRelayRegistrationAtTicks = 0;

        if (cts is not null)
        {
            cts.Cancel();
        }

        udpClient?.Dispose();
        tcpStream?.Dispose();
        tcpClient?.Dispose();
        tcpListener?.Stop();
        TryTerminateAdbShellProcess(adbShellProcess);

        try
        {
            receiveTask?.Wait(400);
            feedbackTask?.Wait(400);
        }
        catch
        {
        }
        ReceiverTrace.Log("Session Stop tasks drained");

        var playbackSyncTaken = false;
        try
        {
            playbackSyncTaken = Monitor.TryEnter(_playbackSync, 300);
            if (!playbackSyncTaken)
            {
                ReceiverTrace.Log("Session Stop playback reset skipped: playback lock timeout");
            }
            else
            {
                _currentSessionConfig = null;
                _playback.PrepareForSessionStop();
            }
        }
        finally
        {
            if (playbackSyncTaken)
            {
                Monitor.Exit(_playbackSync);
            }
        }
        ReceiverTrace.Log("Session Stop playback reset");
        ReceiverTrace.Log("Session Stop audio reset begin");
        _audioReassembler.Reset();
        _audioPlayback.Reset();
        ReceiverTrace.Log("Session Stop audio reset end");
        var playbackBackendLabel = GetPlaybackBackendLabel();

        var syncTaken = false;
        try
        {
            ReceiverTrace.Log("Session Stop snapshot reset begin");
            syncTaken = Monitor.TryEnter(_sync, 300);
            if (!syncTaken)
            {
                ReceiverTrace.Log("Session Stop snapshot reset skipped: session lock timeout");
            }
            else
            {
                _snapshot = new ReceiverSessionSnapshot
                {
                    Status = "Idle",
                    TransportMode = _transportMode.ToUiLabel(),
                    PlaybackBackend = playbackBackendLabel,
                    PlaybackStatus = "Stopped",
                    DecodeMode = _decodeMode.ToUiLabel(),
                    UltraLowLatencyMode = GetEffectiveUltraLowLatencyMode(),
                    SystemHintsEnabled = false,
                };
                _remoteEndpoint = null;
                _inputFrameTicks.Clear();
                _enhancementFrameTicks.Clear();
                _lastVideoArrivalAtTicks = 0;
                _lastFrameDecodedAtTicks = 0;
                _lastFramePresentedAtTicks = 0;
                _lastPresentedBasePresentationTimeUs = 0;
                _lastPresentedBaseTicks = 0;
                _sessionReadyAtTicks = 0;
                _lastForcedCatchUpAtTicks = 0;
                _videoStallLogged = false;
                _decodeStallLogged = false;
                _presentStallLogged = false;
                _baseFrameArrivalTicksByPts.Clear();
                _pendingLatencyPulsesByPts.Clear();
                _recentPulseToPcEstimates.Clear();
                _recentTapToPcEstimates.Clear();
            }
        }
        finally
        {
            if (syncTaken)
            {
                Monitor.Exit(_sync);
            }
        }
        ReceiverTrace.Log("Session Stop snapshot reset end");

        ReceiverTrace.Log("Session Stop performance hints disable begin");
        _performanceHints.Disable();
        ReceiverTrace.Log("Session Stop performance hints disable end");
        ReceiverTrace.Log("Session Stop end");
    }

    public void Dispose()
    {
        Stop();
        var playbackSyncTaken = false;
        try
        {
            playbackSyncTaken = Monitor.TryEnter(_playbackSync, 300);
            if (!playbackSyncTaken)
            {
                ReceiverTrace.Log("Session Dispose skipped playback dispose: playback lock timeout");
            }
            else
            {
                _playback.Dispose();
            }
        }
        finally
        {
            if (playbackSyncTaken)
            {
                Monitor.Exit(_playbackSync);
            }
        }
        _audioPlayback.Dispose();
        _performanceHints.Dispose();
        _controlSendGate.Dispose();
    }

    private async Task ReceiveUdpLoopAsync(CancellationToken token)
    {
        while (!token.IsCancellationRequested)
        {
            UdpReceiveResult result;
            try
            {
                result = await _udpClient!.ReceiveAsync(token);
            }
            catch (OperationCanceledException)
            {
                return;
            }
            catch (ObjectDisposedException)
            {
                return;
            }
            catch (Exception ex)
            {
                lock (_sync)
                {
                    _snapshot = _snapshot with { Status = $"Receive error: {ex.Message}" };
                }
                return;
            }

            try
            {
                HandleIncomingPacket(result.Buffer, result.Buffer.Length, result.RemoteEndPoint);
            }
            catch (Exception ex)
            {
                if (ForceFallbackFromMediaFoundation(ex))
                {
                    continue;
                }
                lock (_sync)
                {
                    _snapshot = _snapshot with
                    {
                    Status = $"Packet handling error: {ex.Message}",
                    PlaybackStatus = "Error",
                    LastPlaybackError = ex.Message,
                    LastBackendFailure = ex.Message,
                };
            }
        }
        }
    }

    private async Task ReceiveTcpLoopAsync(CancellationToken token)
    {
        while (!token.IsCancellationRequested)
        {
            TcpClient client;
            try
            {
                client = await _tcpListener!.AcceptTcpClientAsync(token);
                client.NoDelay = true;
                client.ReceiveBufferSize = 512 * 1024;
                client.SendBufferSize = 64 * 1024;
            }
            catch (OperationCanceledException)
            {
                return;
            }
            catch (ObjectDisposedException)
            {
                return;
            }
            catch (Exception ex)
            {
                lock (_sync)
                {
                    _snapshot = _snapshot with { Status = $"TCP accept error: {ex.Message}" };
                }
                return;
            }

            using (client)
            using (var stream = client.GetStream())
            {
                var remoteEndPoint = client.Client.RemoteEndPoint as IPEndPoint;
                UpdateTcpConnectionState(stream, client, remoteEndPoint, connected: true);

                try
                {
                    while (!token.IsCancellationRequested)
                    {
                        var packetBytes = await TcpPacketFraming.ReadPacketAsync(stream, token);
                        if (packetBytes is null)
                        {
                            break;
                        }

                        try
                        {
                            HandleIncomingPacket(packetBytes, packetBytes.Length, remoteEndPoint);
                        }
                        catch (Exception ex)
                        {
                            if (ForceFallbackFromMediaFoundation(ex))
                            {
                                continue;
                            }
                            lock (_sync)
                            {
                                _snapshot = _snapshot with
                                {
                                    Status = $"Packet handling error: {ex.Message}",
                                    PlaybackStatus = "Error",
                                    LastPlaybackError = ex.Message,
                                    LastBackendFailure = ex.Message,
                                };
                            }
                        }
                    }
                }
                catch (OperationCanceledException)
                {
                    return;
                }
                catch (ObjectDisposedException)
                {
                    return;
                }
                catch (Exception ex)
                {
                    lock (_sync)
                    {
                _snapshot = _snapshot with { Status = $"TCP receive error: {ex.Message}" };
            }
        }
                finally
                {
                    UpdateTcpConnectionState(stream, client, remoteEndPoint, connected: false);
                    TryExecutePlaybackMutation("TCP disconnect keyframe recovery", () => _playback.PrepareForKeyFrameRecovery());
                }
            }
        }
    }

    private async Task ReceiveAdbShellCaptureLoopAsync(int sessionGeneration, CancellationToken token)
    {
        Process? startedProcess = null;
        var adbPath = AdbTunnelManager.ResolveAdbPath();
        var profileResult = AdbTunnelManager.QueryPhysicalDisplaySize(adbPath);
        if (sessionGeneration != Volatile.Read(ref _sessionGeneration))
        {
            return;
        }

        if (!profileResult.Success || profileResult.Profile is null)
        {
            lock (_sync)
            {
                _snapshot = _snapshot with
                {
                    Status = $"ADB shell capture error: {profileResult.Message}",
                    PlaybackStatus = "Error",
                    LastPlaybackError = profileResult.Message,
                    LastBackendFailure = profileResult.Message,
                    RemoteEndpoint = "adb exec-out screenrecord",
                };
            }
            return;
        }

        var profile = profileResult.Profile.Value;
        lock (_sync)
        {
            _snapshot = _snapshot with
            {
                Status = $"ADB shell capture {profile.CaptureWidth}x{profile.CaptureHeight} starting",
                RemoteEndpoint = "adb exec-out screenrecord",
                LastPacketType = "SessionConfig",
                SessionConfigPackets = _snapshot.SessionConfigPackets + 1,
            };
        }

        try
        {
            await AdbShellCaptureTransport.RunAsync(
                adbPath,
                profile,
                onSessionConfig: config => HandleSessionConfig(sessionGeneration, config),
                onAccessUnit: (bytes, isKeyFrame, presentationTimeUs) =>
                    HandleAdbShellAccessUnitReady(sessionGeneration, bytes, isKeyFrame, presentationTimeUs),
                onProcessStarted: process =>
                {
                    startedProcess = process;
                    lock (_sync)
                    {
                        if (sessionGeneration != Volatile.Read(ref _sessionGeneration))
                        {
                            TryTerminateAdbShellProcess(process);
                            return;
                        }

                        _adbShellProcess = process;
                        _snapshot = _snapshot with
                        {
                            Status = $"ADB shell capture active {profile.CaptureWidth}x{profile.CaptureHeight}",
                            RemoteEndpoint = "adb exec-out screenrecord",
                        };
                    }
                },
                cancellationToken: token);
        }
        catch (OperationCanceledException)
        {
        }
        catch (Exception ex)
        {
            if (sessionGeneration != Volatile.Read(ref _sessionGeneration))
            {
                return;
            }

            ReceiverTrace.Log(ex, "ADB shell capture failed");
            lock (_sync)
            {
                _snapshot = _snapshot with
                {
                    Status = $"ADB shell capture failed: {ex.Message}",
                    PlaybackStatus = "Error",
                    LastPlaybackError = ex.Message,
                    LastBackendFailure = ex.Message,
                    RemoteEndpoint = "adb exec-out screenrecord",
                };
            }
        }
        finally
        {
            lock (_sync)
            {
                if (ReferenceEquals(_adbShellProcess, startedProcess))
                {
                    _adbShellProcess = null;
                }
            }
        }
    }

    private void HandleIncomingPacket(byte[] buffer, int length, IPEndPoint? remoteEndPoint)
    {
        if (!ProtocolParser.TryParse(buffer, length, out var packet) || packet is null)
        {
            if (length >= TransportProtocol.HeaderSize)
            {
                var span = buffer.AsSpan(0, length);
                var magic = System.Buffers.Binary.BinaryPrimitives.ReadInt32BigEndian(span[..4]);
                if (magic == TransportProtocol.Magic)
                {
                    var version = span[4];
                    lock (_sync)
                    {
                        _snapshot = _snapshot with
                        {
                            Status = $"Protocol mismatch: EVRT v{version} received, receiver expects v{TransportProtocol.Version}",
                        };
                    }
                }
            }
            return;
        }

        var remoteLabel = remoteEndPoint?.ToString() ??
            (_transportMode == ReceiverTransportMode.AdbTunnelTcp ? "ADB tunnel / TCP" : "-");
        var packetTypeLabel = PacketTypeLabel(packet.Type);
        var sessionConfig = packet.Type == TransportProtocol.TypeSessionConfig
            ? SessionConfig.Parse(packet.Payload)
            : null;
        var latencyPulse = packet.Type == TransportProtocol.TypeControl
            ? LatencyPulseControl.Parse(packet.Payload)
            : null;
        var sessionConfigured = _currentSessionConfig is not null;
        var nowTicks = Stopwatch.GetTimestamp();

        lock (_sync)
        {
            _remoteEndpoint = remoteEndPoint;
            var nextStatus = _snapshot.Status;
            if (!sessionConfigured)
            {
                nextStatus = packet.Type switch
                {
                    TransportProtocol.TypeSessionConfig when sessionConfig is null => "Sender detected, but session config is invalid",
                    TransportProtocol.TypeSessionConfig => "Session config received. Preparing decoder",
                    TransportProtocol.TypeCodecConfig => "Sender detected. Codec config received before session config",
                    TransportProtocol.TypeVideoFrame => "Sender detected. Video packets received before session config",
                    TransportProtocol.TypeEnhancementConfig => "Sender detected. Enhancement config received before session config",
                    TransportProtocol.TypeEnhancementFrame => "Sender detected. Enhancement frame received before session config",
                    TransportProtocol.TypeRoiMetadata => "Sender detected. ROI metadata received before session config",
                    TransportProtocol.TypeAudioConfig or TransportProtocol.TypeAudioFrame => "Sender detected. Audio detected before video session",
                    TransportProtocol.TypeControl => "Sender detected. Control packet received before video session",
                    _ => $"Sender detected. Unknown packet type {packet.Type}; waiting for session config",
                };
            }

            _snapshot = _snapshot with
            {
                PacketsReceived = _snapshot.PacketsReceived + 1,
                RemoteEndpoint = remoteLabel,
                Status = nextStatus,
                LastPacketType = packetTypeLabel,
                SessionConfigPackets = _snapshot.SessionConfigPackets + (packet.Type == TransportProtocol.TypeSessionConfig ? 1 : 0),
                CodecConfigPackets = _snapshot.CodecConfigPackets + (
                    packet.Type == TransportProtocol.TypeCodecConfig || packet.Type == TransportProtocol.TypeEnhancementConfig ? 1 : 0),
                VideoPackets = _snapshot.VideoPackets + (
                    packet.Type == TransportProtocol.TypeVideoFrame || packet.Type == TransportProtocol.TypeEnhancementFrame ? 1 : 0),
                AudioPackets = _snapshot.AudioPackets + (
                    packet.Type == TransportProtocol.TypeAudioConfig || packet.Type == TransportProtocol.TypeAudioFrame ? 1 : 0),
                ControlPackets = _snapshot.ControlPackets + (
                    packet.Type == TransportProtocol.TypeControl || packet.Type == TransportProtocol.TypeRoiMetadata ? 1 : 0),
            };

            if ((packet.Type == TransportProtocol.TypeVideoFrame || packet.Type == TransportProtocol.TypeEnhancementFrame) && packet.PacketIndex == 0)
            {
                _snapshot = _snapshot with
                {
                    ArrivalDeltaMs = ComputeDeltaMs(ref _lastVideoArrivalAtTicks, nowTicks),
                };
            }

            if (packet.Type == TransportProtocol.TypeVideoFrame && packet.PacketIndex == 0 && packet.PresentationTimeUs > 0)
            {
                _baseFrameArrivalTicksByPts[packet.PresentationTimeUs] = nowTicks;
                TrimLatencyPulseStateLocked(packet.PresentationTimeUs);
                TryFinalizeLatencyPulseEstimateLocked(packet.PresentationTimeUs);
            }

            if (latencyPulse is not null)
            {
                _pendingLatencyPulsesByPts[latencyPulse.PresentationTimeUs] = latencyPulse;
                TrimLatencyPulseStateLocked(latencyPulse.PresentationTimeUs);
                TryFinalizeLatencyPulseEstimateLocked(latencyPulse.PresentationTimeUs);
            }
        }

        if (packet.Type == TransportProtocol.TypeAudioConfig)
        {
            var audioConfig = ReceiverAudioConfig.Parse(packet.Payload);
            if (audioConfig is not null)
            {
                _audioPlayback.ApplyConfig(audioConfig);
                _audioReassembler.Reset();
            }
            return;
        }

        if (packet.Type == TransportProtocol.TypeAudioFrame)
        {
            _audioReassembler.OnPacket(packet);
            return;
        }

        if (packet.Type == TransportProtocol.TypeControl)
        {
            return;
        }

        _reassembler?.OnPacket(packet);
    }

    private void TrimLatencyPulseStateLocked(long newestPresentationTimeUs)
    {
        var staleThresholdUs = newestPresentationTimeUs - 2_000_000L;
        foreach (var staleKey in _baseFrameArrivalTicksByPts.Keys.Where(key => key < staleThresholdUs).ToArray())
        {
            _baseFrameArrivalTicksByPts.Remove(staleKey);
        }

        foreach (var staleKey in _pendingLatencyPulsesByPts.Keys.Where(key => key < staleThresholdUs).ToArray())
        {
            _pendingLatencyPulsesByPts.Remove(staleKey);
        }

        while (_baseFrameArrivalTicksByPts.Count > 96)
        {
            var firstKey = _baseFrameArrivalTicksByPts.Keys.Min();
            _baseFrameArrivalTicksByPts.Remove(firstKey);
        }

        while (_pendingLatencyPulsesByPts.Count > 32)
        {
            var firstKey = _pendingLatencyPulsesByPts.Keys.Min();
            _pendingLatencyPulsesByPts.Remove(firstKey);
        }
    }

    private void TryFinalizeLatencyPulseEstimateLocked(long presentationTimeUs)
    {
        if (!_pendingLatencyPulsesByPts.TryGetValue(presentationTimeUs, out var pulse))
        {
            return;
        }

        if (!_baseFrameArrivalTicksByPts.TryGetValue(presentationTimeUs, out var arrivalTicks))
        {
            return;
        }

        if (_lastPresentedBasePresentationTimeUs != presentationTimeUs || _lastPresentedBaseTicks == 0)
        {
            return;
        }

        var receiverTailMs = (int)Math.Round(ElapsedSince(arrivalTicks, _lastPresentedBaseTicks).TotalMilliseconds);
        if (receiverTailMs < 0)
        {
            receiverTailMs = 0;
        }

        _pendingLatencyPulsesByPts.Remove(presentationTimeUs);
        _baseFrameArrivalTicksByPts.Remove(presentationTimeUs);
        var pulseToPcEstimateMs = pulse.SenderPipelineMs + receiverTailMs;
        var tapToPcEstimateMs = pulse.ApproxSenderMs + receiverTailMs;
        if (!IsLatencyEstimateStableLocked())
        {
            ReceiverTrace.Log($"Latency sample ignored during startup/instability: pulse={pulseToPcEstimateMs} ms, tap={tapToPcEstimateMs} ms, fps={_snapshot.InputFpsProxy}, frames={_snapshot.FramesAssembled}, queueDrops={_snapshot.StreamDroppedAccessUnits}");
            return;
        }

        PushLatencyEstimate(_recentPulseToPcEstimates, pulseToPcEstimateMs);
        PushLatencyEstimate(_recentTapToPcEstimates, tapToPcEstimateMs);
        _snapshot = _snapshot with
        {
            PulseToPcEstimateMs = ComputeMedian(_recentPulseToPcEstimates),
            TapToPcEstimateMs = ComputeMedian(_recentTapToPcEstimates),
        };
    }

    private bool IsLatencyEstimateStableLocked()
    {
        if (!_snapshot.PlaybackStatus.StartsWith("Playing", StringComparison.OrdinalIgnoreCase))
        {
            return false;
        }

        if (_snapshot.WaitingForKeyFrame)
        {
            return false;
        }

        var targetFps = _snapshot.TargetFps;
        if (targetFps > 0)
        {
            if (_snapshot.InputFpsProxy < Math.Max(12, (int)Math.Round(targetFps * 0.70)))
            {
                return false;
            }

            if (_snapshot.FramesAssembled < Math.Max(targetFps, 60))
            {
                return false;
            }
        }

        return true;
    }

    private static void PushLatencyEstimate(Queue<int> queue, int value)
    {
        queue.Enqueue(Math.Max(0, value));
        while (queue.Count > 7)
        {
            queue.Dequeue();
        }
    }

    private static int ComputeMedian(Queue<int> queue)
    {
        if (queue.Count == 0)
        {
            return -1;
        }

        var ordered = queue.OrderBy(static value => value).ToArray();
        return ordered[ordered.Length / 2];
    }

    private void UpdateTcpConnectionState(
        NetworkStream stream,
        TcpClient client,
        IPEndPoint? remoteEndPoint,
        bool connected)
    {
        lock (_sync)
        {
            if (connected)
            {
                _tcpStream = stream;
                _tcpClient = client;
                _remoteEndpoint = remoteEndPoint;
                _snapshot = _snapshot with
                {
                    Status = $"TCP sender connected on {_listeningPort}",
                    RemoteEndpoint = remoteEndPoint?.ToString() ?? "ADB tunnel / TCP",
                };
            }
            else
            {
                if (ReferenceEquals(_tcpStream, stream))
                {
                    _tcpStream = null;
                }
                if (ReferenceEquals(_tcpClient, client))
                {
                    _tcpClient = null;
                }
                _remoteEndpoint = null;
                if (_snapshot.Listening)
                {
                    _snapshot = _snapshot with
                    {
                        Status = _transportMode.BuildWaitingStatus(_listeningPort),
                        RemoteEndpoint = "-",
                    };
                }
            }
        }
    }

    private async Task FeedbackLoopAsync(CancellationToken token)
    {
        using var timer = new PeriodicTimer(TimeSpan.FromMilliseconds(_ultraLowLatencyMode ? 70 : (_aggressiveMode ? 100 : 175)));
        while (await timer.WaitForNextTickAsync(token))
        {
            byte[]? feedback = null;
            byte[]? requestKeyFrame = null;
            byte[]? relayRegistration = null;
            int? desiredJitterMs = null;

            lock (_sync)
            {
                var nowTicks = Stopwatch.GetTimestamp();
                var transportReady = _transportMode == ReceiverTransportMode.Udp
                    ? _udpClient is not null && (_remoteEndpoint is not null || _relayRoute is not null || _relayRegistrationRoute is not null)
                    : _tcpStream is not null && _tcpClient is not null;
                if (!transportReady || !_snapshot.Listening)
                {
                    continue;
                }

                if (_transportMode == ReceiverTransportMode.Udp &&
                    _relayRegistrationRoute is not null &&
                    ElapsedSince(_lastRelayRegistrationAtTicks, nowTicks) >= TimeSpan.FromSeconds(2))
                {
                    relayRegistration = BuildRelayRegistrationPacket();
                    _lastRelayRegistrationAtTicks = nowTicks;
                }

                UpdateStallDiagnostics(nowTicks);
                var feedbackCooldown = TimeSpan.FromMilliseconds(GetEffectiveFeedbackTickMs(cinemaSmooth: _currentSessionConfig?.IsCinemaSmooth ?? false));
                if (ElapsedSince(_lastFeedbackAtTicks, nowTicks) < feedbackCooldown)
                {
                    goto ExitFeedbackBuild;
                }

                var cinemaSmooth = _currentSessionConfig?.IsCinemaSmooth ?? false;

                var startupGrace =
                    _sessionReadyAtTicks != 0 &&
                    ElapsedSince(_sessionReadyAtTicks, nowTicks) < TimeSpan.FromMilliseconds(GetEffectiveStartupGraceMs(cinemaSmooth)) &&
                    _snapshot.FramesAssembled < (cinemaSmooth ? 36 : (_ultraLowLatencyMode ? 18 : 24));

                var playbackBuffering =
                    _snapshot.PlaybackStatus.StartsWith("Buffering", StringComparison.OrdinalIgnoreCase) ||
                    _snapshot.PlaybackStatus.StartsWith("Opening", StringComparison.OrdinalIgnoreCase);
                var playbackRecoveryPressure = playbackBuffering || (!cinemaSmooth && _snapshot.WaitingForKeyFrame);
                var queuedFrames = _snapshot.StreamQueuedAccessUnits;
                var hasLanBacklog = _transportMode == ReceiverTransportMode.AdbTunnelTcp
                    ? queuedFrames > (cinemaSmooth ? 1 : 0)
                    : queuedFrames > (cinemaSmooth ? 2 : 1);
                var transportBacklogCritical =
                    _transportMode == ReceiverTransportMode.AdbTunnelTcp &&
                    queuedFrames > (cinemaSmooth ? 1 : 0);
                var criticalAssemblyDelayMs = _transportMode == ReceiverTransportMode.AdbTunnelTcp
                    ? (cinemaSmooth ? 34 : (_ultraLowLatencyMode ? 18 : 28))
                    : (cinemaSmooth ? 22 : (_ultraLowLatencyMode ? 10 : 18));
                var highAssemblyDelayMs = _transportMode == ReceiverTransportMode.AdbTunnelTcp
                    ? (cinemaSmooth ? 22 : (_ultraLowLatencyMode ? 10 : 18))
                    : (cinemaSmooth ? 14 : (_ultraLowLatencyMode ? 6 : 12));
                var highCadenceDeltaMs = GetEffectiveHighDeltaMs(cinemaSmooth);
                var criticalCadenceDeltaMs = GetEffectiveCriticalDeltaMs(cinemaSmooth);
                var computedHighCadenceDeltaMs = _transportMode == ReceiverTransportMode.AdbTunnelTcp
                    ? (cinemaSmooth ? 42 : (_ultraLowLatencyMode ? 28 : 34))
                    : (cinemaSmooth ? 30 : (_ultraLowLatencyMode ? 22 : 28));
                var computedCriticalCadenceDeltaMs = _transportMode == ReceiverTransportMode.AdbTunnelTcp
                    ? (cinemaSmooth ? 60 : (_ultraLowLatencyMode ? 42 : 52))
                    : (cinemaSmooth ? 44 : (_ultraLowLatencyMode ? 34 : 42));
                if (highCadenceDeltaMs <= 0)
                {
                    highCadenceDeltaMs = computedHighCadenceDeltaMs;
                }
                if (criticalCadenceDeltaMs <= 0)
                {
                    criticalCadenceDeltaMs = computedCriticalCadenceDeltaMs;
                }
                if (criticalCadenceDeltaMs < highCadenceDeltaMs)
                {
                    criticalCadenceDeltaMs = highCadenceDeltaMs;
                }
                var highCadenceSpike =
                    (_snapshot.ArrivalDeltaMs >= highCadenceDeltaMs && _snapshot.ArrivalDeltaMs >= 0) ||
                    (_snapshot.DecodeDeltaMs >= highCadenceDeltaMs && _snapshot.DecodeDeltaMs >= 0) ||
                    (_snapshot.PresentDeltaMs >= highCadenceDeltaMs && _snapshot.PresentDeltaMs >= 0);
                var criticalCadenceSpike =
                    (_snapshot.ArrivalDeltaMs >= criticalCadenceDeltaMs && _snapshot.ArrivalDeltaMs >= 0) ||
                    (_snapshot.DecodeDeltaMs >= criticalCadenceDeltaMs && _snapshot.DecodeDeltaMs >= 0) ||
                    (_snapshot.PresentDeltaMs >= criticalCadenceDeltaMs && _snapshot.PresentDeltaMs >= 0);
                var criticalDropBurst =
                    !startupGrace &&
                    _snapshot.StreamDroppedAccessUnits >
                    _lastFeedbackDrops + GetEffectiveDropBurstStep(cinemaSmooth, critical: true);
                var highDropBurst = !startupGrace && _snapshot.StreamDroppedAccessUnits > _lastFeedbackDrops;
                var criticalLowInputFps =
                    _snapshot.TargetFps >= 24 &&
                    _snapshot.InputFpsProxy > 0 &&
                    !startupGrace &&
                    (hasLanBacklog || playbackRecoveryPressure || highDropBurst) &&
                    _snapshot.InputFpsProxy <
                    (cinemaSmooth
                        ? (int)(_snapshot.TargetFps * 0.30f)
                        : (_aggressiveMode ? (int)(_snapshot.TargetFps * 0.45f) : (int)(_snapshot.TargetFps * 0.40f)));
                var highLowInputFps =
                    _snapshot.TargetFps >= 24 &&
                    _snapshot.InputFpsProxy > 0 &&
                    !startupGrace &&
                    (_transportMode == ReceiverTransportMode.AdbTunnelTcp ||
                     hasLanBacklog ||
                     playbackRecoveryPressure ||
                     highDropBurst ||
                     _snapshot.AssemblyDelayMs >= highAssemblyDelayMs) &&
                    _snapshot.InputFpsProxy <
                    (cinemaSmooth
                        ? (int)(_snapshot.TargetFps * 0.60f)
                        : (_aggressiveMode ? (int)(_snapshot.TargetFps * 0.80f) : (int)(_snapshot.TargetFps * 0.70f)));
                var criticalPressure =
                    (transportBacklogCritical && !cinemaSmooth) ||
                    _snapshot.AssemblyDelayMs >= criticalAssemblyDelayMs ||
                    criticalCadenceSpike ||
                    queuedFrames > GetEffectivePanicQueueThresholdAu(cinemaSmooth) ||
                    criticalDropBurst ||
                    criticalLowInputFps;

                var highPressure =
                    criticalPressure ||
                    playbackRecoveryPressure ||
                    _snapshot.AssemblyDelayMs >= highAssemblyDelayMs ||
                    highCadenceSpike ||
                    (hasLanBacklog && !startupGrace) ||
                    highDropBurst ||
                    highLowInputFps;

                var pressure = criticalPressure ? "critical" : highPressure ? "high" : "normal";
                desiredJitterMs = ComputeAdaptiveJitterMs(
                    cinemaSmooth,
                    startupGrace,
                    highCadenceSpike,
                    criticalCadenceSpike,
                    highDropBurst,
                    criticalDropBurst,
                    queuedFrames,
                    _snapshot.ArrivalDeltaMs,
                    _snapshot.DecodeDeltaMs,
                    _snapshot.PresentDeltaMs);
                if (desiredJitterMs.Value != _adaptiveJitterMs)
                {
                    _adaptiveJitterMs = desiredJitterMs.Value;
                    _snapshot = _snapshot with { AdaptiveJitterMs = _adaptiveJitterMs };
                }

                feedback = ControlPacketBuilder.BuildReceiverFeedback(
                    pressure: pressure,
                    backlogFrames: _snapshot.StreamQueuedAccessUnits,
                    queueDrops: _snapshot.StreamDroppedAccessUnits,
                    decodeFps: _snapshot.InputFpsProxy,
                    assemblyDelayMs: _snapshot.AssemblyDelayMs,
                    arrivalDeltaMs: _snapshot.ArrivalDeltaMs,
                    decodeDeltaMs: _snapshot.DecodeDeltaMs,
                    presentDeltaMs: _snapshot.PresentDeltaMs,
                    pulseEstimateMs: _snapshot.PulseToPcEstimateMs,
                    inputEstimateMs: _snapshot.TapToPcEstimateMs);
                _lastFeedbackAtTicks = nowTicks;

                var networkStallCatchUp =
                    _transportMode == ReceiverTransportMode.Udp &&
                    _ultraLowLatencyMode &&
                    !cinemaSmooth &&
                    !startupGrace &&
                    _snapshot.ArrivalDeltaMs >= GetEffectiveCatchUpThresholdMs() &&
                    _snapshot.ArrivalDeltaMs >= 0;
                if (networkStallCatchUp &&
                    ElapsedSince(_lastForcedCatchUpAtTicks, nowTicks) >= TimeSpan.FromMilliseconds(GetEffectiveForcedCatchUpCooldownMs()))
                {
                    TryExecutePlaybackMutation("Feedback keyframe recovery", () => _playback.PrepareForKeyFrameRecovery());
                    requestKeyFrame = ControlPacketBuilder.BuildRequestKeyFrame();
                    _lastRequestKeyFrameAtTicks = nowTicks;
                    _lastForcedCatchUpAtTicks = nowTicks;
                }

                if (requestKeyFrame is null &&
                    highPressure &&
                    ElapsedSince(_lastRequestKeyFrameAtTicks, nowTicks) >= TimeSpan.FromMilliseconds(GetEffectiveKeyFrameCooldownMs(cinemaSmooth, criticalPressure)))
                {
                    requestKeyFrame = ControlPacketBuilder.BuildRequestKeyFrame();
                    _lastRequestKeyFrameAtTicks = nowTicks;
                }

                _lastFeedbackDrops = _snapshot.StreamDroppedAccessUnits;

ExitFeedbackBuild:;
            }

            if (desiredJitterMs.HasValue)
            {
                TryExecutePlaybackMutation(
                    "Feedback adaptive jitter update",
                    () => _playback.UpdateAdaptiveJitterBuffer(TimeSpan.FromMilliseconds(desiredJitterMs.Value)));
            }

            try
            {
                if (relayRegistration is not null)
                {
                    await SendRelayRegistrationPacketAsync(relayRegistration, token);
                }
                if (feedback is not null)
                {
                    await SendControlPacketAsync(feedback, token);
                }
                if (requestKeyFrame is not null)
                {
                    await SendControlPacketAsync(requestKeyFrame, token);
                }
            }
            catch
            {
            }
        }
    }

    private void HandleSessionConfig(int sessionGeneration, SessionConfig config)
    {
        if (sessionGeneration != Volatile.Read(ref _sessionGeneration))
        {
            return;
        }

        SessionConfig? previousConfig;
        lock (_sync)
        {
            previousConfig = _currentSessionConfig;
        }

        var changeSummary = DescribeSessionConfigChange(previousConfig, config);
        ReceiverTrace.Log(previousConfig is null
            ? $"Session config received: {changeSummary}"
            : $"Session config changed: {changeSummary}");

        var requiresPlaybackReconfigure =
            previousConfig is null ||
            !string.Equals(previousConfig.Codec, config.Codec, StringComparison.OrdinalIgnoreCase) ||
            !string.Equals(previousConfig.Transport, config.Transport, StringComparison.OrdinalIgnoreCase) ||
            previousConfig.Width != config.Width ||
            previousConfig.Height != config.Height ||
            previousConfig.Fps != config.Fps ||
            previousConfig.IsSplitStream != config.IsSplitStream ||
            previousConfig.EnhancementMaxWidth != config.EnhancementMaxWidth ||
            previousConfig.EnhancementMaxHeight != config.EnhancementMaxHeight ||
            !string.Equals(previousConfig.EnhancementCodec, config.EnhancementCodec, StringComparison.OrdinalIgnoreCase);

        if (!requiresPlaybackReconfigure)
        {
            _audioPlayback.UpdateCinemaSmoothMode(config.IsCinemaSmooth);
            var playbackBackendLabel = GetPlaybackBackendLabel();
            lock (_sync)
            {
                _currentSessionConfig = config;
                _snapshot = _snapshot with
                {
                    Status = $"Receiving {CodecLabel(config.Codec)} {config.ResolutionLabel}",
                    PlaybackBackend = playbackBackendLabel,
                    StreamMode = config.StreamMode,
                    Codec = CodecLabel(config.Codec),
                    Preset = config.Preset,
                    Resolution = config.ResolutionLabel,
                    TargetFps = config.Fps,
                    BitrateMbps = config.Bitrate / 1_000_000.0,
                    AdaptiveJitterMs = GetEffectiveAdaptiveJitterMs(),
                };
            }
            return;
        }

        Exception? applyError = null;
        ExecutePlaybackMutation("HandleSessionConfig", () =>
        {
            _currentSessionConfig = config;
            _audioPlayback.UpdateCinemaSmoothMode(config.IsCinemaSmooth);
            try
            {
                _playback.UpdateAggressiveMode(GetEffectiveAggressiveMode());
                _playback.UpdateUltraLowLatencyMode(GetEffectiveUltraLowLatencyMode());
                _playback.UpdateAdaptiveJitterBuffer(GetAdaptiveJitterDelay());
                _playback.UpdatePacingWindow(_manualMinPacingDelay, _manualMaxPacingDelay);
                _playback.ApplySessionConfig(config);
            }
            catch (Exception ex)
            {
                applyError = ex;
            }
        });

        if (applyError is not null)
        {
            if (TryFallbackFromMediaFoundation(config, applyError))
            {
                return;
            }

            lock (_sync)
            {
                _snapshot = _snapshot with
                {
                    Status = $"Playback init error: {applyError.Message}",
                    PlaybackStatus = "Error",
                    LastPlaybackError = applyError.Message,
                    LastBackendFailure = applyError.Message,
                    Codec = CodecLabel(config.Codec),
                    Preset = config.Preset,
                    Resolution = config.ResolutionLabel,
                    TargetFps = config.Fps,
                    BitrateMbps = config.Bitrate / 1_000_000.0,
                };
            }
            return;
        }

        var currentPlaybackBackendLabel = GetPlaybackBackendLabel();
        lock (_sync)
        {
            _snapshot = _snapshot with
            {
                Status = $"Receiving {CodecLabel(config.Codec)} {config.ResolutionLabel}",
                PlaybackBackend = currentPlaybackBackendLabel,
                StreamMode = config.StreamMode,
                Codec = CodecLabel(config.Codec),
                Preset = config.Preset,
                Resolution = config.ResolutionLabel,
                TargetFps = config.Fps,
                BitrateMbps = config.Bitrate / 1_000_000.0,
                AdaptiveJitterMs = GetEffectiveAdaptiveJitterMs(),
                UltraLowLatencyMode = GetEffectiveUltraLowLatencyMode(),
            };
            _sessionReadyAtTicks = Stopwatch.GetTimestamp();
            _inputFrameTicks.Clear();
            _enhancementFrameTicks.Clear();
            _lastVideoArrivalAtTicks = 0;
            _lastFrameDecodedAtTicks = 0;
            _lastFramePresentedAtTicks = 0;
            _lastPresentedBasePresentationTimeUs = 0;
            _lastPresentedBaseTicks = 0;
            _videoStallLogged = false;
            _decodeStallLogged = false;
            _presentStallLogged = false;
            _baseFrameArrivalTicksByPts.Clear();
            _pendingLatencyPulsesByPts.Clear();
            _recentPulseToPcEstimates.Clear();
            _recentTapToPcEstimates.Clear();
        }

        RequestKeyFrame();
    }

    private bool TryFallbackFromMediaFoundation(SessionConfig config, Exception originalError)
    {
        if (_playbackBackend != PlaybackBackendKind.MediaFoundationDirect3D11)
        {
            return false;
        }

        IPlaybackController? previousPlayback = null;
        IPlaybackController? fallbackPlayback = null;
        try
        {
            fallbackPlayback = RunOnPlaybackHostThread(() => CreatePlaybackController(PlaybackBackendKind.LibVlcHwndDirect3D11));
            fallbackPlayback.UpdateHardwareDecodeMode(_decodeMode);
            fallbackPlayback.UpdateAggressiveMode(GetEffectiveAggressiveMode());
            fallbackPlayback.UpdateUltraLowLatencyMode(GetEffectiveUltraLowLatencyMode());
            fallbackPlayback.UpdateAdaptiveJitterBuffer(GetAdaptiveJitterDelay());
            fallbackPlayback.UpdatePacingWindow(_manualMinPacingDelay, _manualMaxPacingDelay);
            fallbackPlayback.ApplySessionConfig(config);
            fallbackPlayback.WaitForKeyFrame();

            ExecutePlaybackMutation("TryFallbackFromMediaFoundation swap", () =>
            {
                if (_playbackBackend != PlaybackBackendKind.MediaFoundationDirect3D11)
                {
                    fallbackPlayback.Dispose();
                    return;
                }

                previousPlayback = _playback;
                _playback = fallbackPlayback;
                _activePlaybackRef = fallbackPlayback;
                _playbackBackend = PlaybackBackendKind.LibVlcHwndDirect3D11;
                _playbackBackendLabel = _playback.BackendLabel;
                _currentSessionConfig = config;
                fallbackPlayback = null;
            });

            previousPlayback?.Dispose();

            lock (_sync)
            {
                _snapshot = _snapshot with
                {
                    Status = $"MF init failed, fallback to LibVLC HWND + D3D11",
                    PlaybackBackend = PlaybackBackendKind.LibVlcHwndDirect3D11.ToUiLabel(),
                    PlaybackStatus = "Opening",
                    LastPlaybackError = "-",
                    LastBackendFailure = originalError.Message,
                    Codec = CodecLabel(config.Codec),
                    Preset = config.Preset,
                    Resolution = config.ResolutionLabel,
                    TargetFps = config.Fps,
                    BitrateMbps = config.Bitrate / 1_000_000.0,
                    DecodeDeltaMs = -1,
                    PresentDeltaMs = -1,
                AdaptiveJitterMs = _adaptiveJitterMs,
            };
            _inputFrameTicks.Clear();
            _enhancementFrameTicks.Clear();
            _lastFrameDecodedAtTicks = 0;
            _lastFramePresentedAtTicks = 0;
            _lastPresentedBasePresentationTimeUs = 0;
            _lastPresentedBaseTicks = 0;
            _baseFrameArrivalTicksByPts.Clear();
            _pendingLatencyPulsesByPts.Clear();
            _recentPulseToPcEstimates.Clear();
            _recentTapToPcEstimates.Clear();
            }
            RequestKeyFrame();
            return true;
        }
        catch (Exception fallbackError)
        {
            fallbackPlayback?.Dispose();
            lock (_sync)
            {
                _snapshot = _snapshot with
                {
                    Status = $"Playback init error: {originalError.Message}; fallback failed: {fallbackError.Message}",
                    PlaybackStatus = "Error",
                    LastPlaybackError = $"{originalError.Message}; fallback failed: {fallbackError.Message}",
                    LastBackendFailure = $"{originalError.Message}; fallback failed: {fallbackError.Message}",
                    Codec = CodecLabel(config.Codec),
                    Preset = config.Preset,
                    Resolution = config.ResolutionLabel,
                    TargetFps = config.Fps,
                    BitrateMbps = config.Bitrate / 1_000_000.0,
                };
            }
            return false;
        }
    }

    private bool TryFallbackFromMediaFoundation(Exception originalError)
    {
        var config = _currentSessionConfig;
        return config is not null && TryFallbackFromMediaFoundation(config, originalError);
    }

    private bool ForceFallbackFromMediaFoundation(Exception originalError)
    {
        var currentBackend = _playbackBackend;
        var config = _currentSessionConfig;

        if (currentBackend != PlaybackBackendKind.MediaFoundationDirect3D11 || config is null)
        {
            return false;
        }

        try
        {
            UpdatePlaybackBackend(PlaybackBackendKind.LibVlcHwndDirect3D11);
            lock (_sync)
            {
                _snapshot = _snapshot with
                {
                    Status = $"MF runtime failed, forced fallback to LibVLC HWND + D3D11",
                    PlaybackBackend = PlaybackBackendKind.LibVlcHwndDirect3D11.ToUiLabel(),
                    PlaybackStatus = "Opening",
                    LastPlaybackError = "-",
                    LastBackendFailure = originalError.Message,
                    Codec = CodecLabel(config.Codec),
                    Preset = config.Preset,
                    Resolution = config.ResolutionLabel,
                    TargetFps = config.Fps,
                    BitrateMbps = config.Bitrate / 1_000_000.0,
                    DecodeDeltaMs = -1,
                    PresentDeltaMs = -1,
                AdaptiveJitterMs = _adaptiveJitterMs,
            };
            _inputFrameTicks.Clear();
            _enhancementFrameTicks.Clear();
            _lastFrameDecodedAtTicks = 0;
            _lastFramePresentedAtTicks = 0;
            _lastPresentedBasePresentationTimeUs = 0;
            _lastPresentedBaseTicks = 0;
            _baseFrameArrivalTicksByPts.Clear();
            _pendingLatencyPulsesByPts.Clear();
            _recentPulseToPcEstimates.Clear();
            _recentTapToPcEstimates.Clear();
            }
            RequestKeyFrame();
            return true;
        }
        catch (Exception fallbackError)
        {
            lock (_sync)
            {
                _snapshot = _snapshot with
                {
                    Status = $"MF runtime failed: {originalError.Message}; forced fallback failed: {fallbackError.Message}",
                    PlaybackStatus = "Error",
                    LastPlaybackError = $"{originalError.Message}; forced fallback failed: {fallbackError.Message}",
                    LastBackendFailure = $"{originalError.Message}; forced fallback failed: {fallbackError.Message}",
                };
            }
            return false;
        }
    }

    private void HandleAdbShellAccessUnitReady(int sessionGeneration, byte[] bytes, bool isKeyFrame, long presentationTimeUs)
    {
        if (sessionGeneration != Volatile.Read(ref _sessionGeneration))
        {
            return;
        }

        var nowTicks = Stopwatch.GetTimestamp();
        lock (_sync)
        {
            _snapshot = _snapshot with
            {
                PacketsReceived = _snapshot.PacketsReceived + 1,
                VideoPackets = _snapshot.VideoPackets + 1,
                LastPacketType = "VideoFrame",
                RemoteEndpoint = "adb exec-out screenrecord",
                ArrivalDeltaMs = ComputeDeltaMs(ref _lastVideoArrivalAtTicks, nowTicks),
            };

            if (presentationTimeUs > 0)
            {
                _baseFrameArrivalTicksByPts[presentationTimeUs] = nowTicks;
                TrimLatencyPulseStateLocked(presentationTimeUs);
                TryFinalizeLatencyPulseEstimateLocked(presentationTimeUs);
            }
        }

        HandleAccessUnitReady(sessionGeneration, bytes, isKeyFrame, assemblyDelayMs: 0, presentationTimeUs);
    }

    private void HandleAccessUnitReady(int sessionGeneration, byte[] bytes, bool isKeyFrame, int assemblyDelayMs, long presentationTimeUs)
    {
        if (sessionGeneration != Volatile.Read(ref _sessionGeneration))
        {
            return;
        }

        try
        {
            var entered = false;
            try
            {
                entered = Monitor.TryEnter(_playbackSync, 100);
                if (!entered)
                {
                    ReceiverTrace.Log("Base access-unit enqueue skipped: playback lock timeout");
                    return;
                }

                _playback.EnqueueAccessUnit(bytes, isKeyFrame, presentationTimeUs);
            }
            finally
            {
                if (entered)
                {
                    Monitor.Exit(_playbackSync);
                }
            }
        }
        catch (Exception ex)
        {
            if (ForceFallbackFromMediaFoundation(ex))
            {
                return;
            }
            throw;
        }

        lock (_sync)
        {
            var framesAssembled = _snapshot.FramesAssembled + 1;
            var nowTicks = Stopwatch.GetTimestamp();
            _inputFrameTicks.Enqueue(nowTicks);
            var minTicks = nowTicks - Stopwatch.Frequency;
            while (_inputFrameTicks.Count > 1 && _inputFrameTicks.Peek() < minTicks)
            {
                _inputFrameTicks.Dequeue();
            }

            var inputFpsProxy = _snapshot.InputFpsProxy;
            if (_inputFrameTicks.Count >= 2)
            {
                var firstTick = _inputFrameTicks.Peek();
                var seconds = (nowTicks - firstTick) / (double)Stopwatch.Frequency;
                if (seconds >= 0.15 && _inputFrameTicks.Count >= 4)
                {
                    inputFpsProxy = (int)Math.Round((_inputFrameTicks.Count - 1) / seconds);
                    if (_snapshot.TargetFps > 0)
                    {
                        inputFpsProxy = Math.Clamp(inputFpsProxy, 1, _snapshot.TargetFps * 2);
                    }
                }
            }
            else if (_inputFrameTicks.Count == 1)
            {
                inputFpsProxy = 1;
            }

            _snapshot = _snapshot with
            {
                FramesAssembled = framesAssembled,
                InputFpsProxy = inputFpsProxy,
                AssemblyDelayMs = assemblyDelayMs,
                Status = isKeyFrame ? "Keyframe received" : _snapshot.Status,
            };
        }
    }

    private void HandleEnhancementAccessUnitReady(int sessionGeneration, EnhancementAccessUnit accessUnit)
    {
        if (sessionGeneration != Volatile.Read(ref _sessionGeneration))
        {
            return;
        }

        try
        {
            var entered = false;
            try
            {
                entered = Monitor.TryEnter(_playbackSync, 100);
                if (!entered)
                {
                    ReceiverTrace.Log("Enhancement access-unit enqueue skipped: playback lock timeout");
                    return;
                }

                _playback.EnqueueEnhancementAccessUnit(
                    accessUnit.Bytes,
                    accessUnit.IsKeyFrame,
                    accessUnit.PresentationTimeUs,
                    accessUnit.Metadata);
            }
            finally
            {
                if (entered)
                {
                    Monitor.Exit(_playbackSync);
                }
            }
        }
        catch (Exception ex)
        {
            if (ForceFallbackFromMediaFoundation(ex))
            {
                return;
            }
            throw;
        }

        lock (_sync)
        {
            var nowTicks = Stopwatch.GetTimestamp();
            _enhancementFrameTicks.Enqueue(nowTicks);
            var minTicks = nowTicks - Stopwatch.Frequency;
            while (_enhancementFrameTicks.Count > 1 && _enhancementFrameTicks.Peek() < minTicks)
            {
                _enhancementFrameTicks.Dequeue();
            }

            var enhancementFps = _snapshot.EnhancementFps;
            if (_enhancementFrameTicks.Count >= 2)
            {
                var firstTick = _enhancementFrameTicks.Peek();
                var seconds = (nowTicks - firstTick) / (double)Stopwatch.Frequency;
                if (seconds >= 0.15 && _enhancementFrameTicks.Count >= 4)
                {
                    enhancementFps = (int)Math.Round((_enhancementFrameTicks.Count - 1) / seconds);
                    enhancementFps = Math.Clamp(enhancementFps, 1, Math.Max(1, _snapshot.TargetFps * 2));
                }
            }
            else if (_enhancementFrameTicks.Count == 1)
            {
                enhancementFps = 1;
            }

            _snapshot = _snapshot with
            {
                EnhancementFramesAssembled = _snapshot.EnhancementFramesAssembled + 1,
                EnhancementFps = enhancementFps,
                RoiActive = accessUnit.Metadata is not null,
                RoiRect = accessUnit.Metadata?.RectLabel ?? _snapshot.RoiRect,
            };
        }
    }

    private async Task SendControlPacketAsync(byte[] packet, CancellationToken cancellationToken)
    {
        var entered = false;
        try
        {
            await _controlSendGate.WaitAsync(cancellationToken);
            entered = true;

            if (_transportMode == ReceiverTransportMode.Udp)
            {
                UdpClient? client;
                IPEndPoint? endpoint;
                lock (_sync)
                {
                    client = _udpClient;
                    endpoint = _relayRoute?.ToEndPoint() ?? _remoteEndpoint;
                }

                if (client is null || endpoint is null)
                {
                    return;
                }

                await client.SendAsync(packet, endpoint, cancellationToken);
                return;
            }

            NetworkStream? stream;
            lock (_sync)
            {
                stream = _tcpStream;
            }

            if (stream is null)
            {
                return;
            }

            await TcpPacketFraming.WritePacketAsync(stream, packet, cancellationToken);
        }
        finally
        {
            if (entered)
            {
                _controlSendGate.Release();
            }
        }
    }

    private async Task SendRelayRegistrationPacketAsync(byte[] packet, CancellationToken cancellationToken)
    {
        if (_transportMode != ReceiverTransportMode.Udp)
        {
            await SendControlPacketAsync(packet, cancellationToken);
            return;
        }

        var entered = false;
        try
        {
            await _controlSendGate.WaitAsync(cancellationToken);
            entered = true;

            UdpClient? client;
            IPEndPoint? endpoint;
            lock (_sync)
            {
                client = _udpClient;
                endpoint = _relayRegistrationRoute?.ToEndPoint();
            }

            if (client is null || endpoint is null)
            {
                return;
            }

            await client.SendAsync(packet, endpoint, cancellationToken);
        }
        finally
        {
            if (entered)
            {
                _controlSendGate.Release();
            }
        }
    }

    private void RequestKeyFrame()
    {
        if (_transportMode == ReceiverTransportMode.AdbShellH264)
        {
            return;
        }

        if (!GetSnapshot().Listening)
        {
            return;
        }

        var packet = ControlPacketBuilder.BuildRequestKeyFrame();
        _ = Task.Run(async () =>
        {
            try
            {
                await SendControlPacketAsync(packet, CancellationToken.None);
            }
            catch
            {
            }
        });
    }

    private void SendControlPacketFireAndForget(byte[] packet)
    {
        if (_transportMode == ReceiverTransportMode.AdbShellH264 || !GetSnapshot().Listening)
        {
            return;
        }

        _ = Task.Run(
            async () =>
            {
                try
                {
                    await SendControlPacketAsync(packet, CancellationToken.None);
                }
                catch
                {
                }
            });
    }

    private byte[]? BuildRelayRegistrationPacket()
    {
        var route = _relayRegistrationRoute;
        if (route is null)
        {
            return null;
        }

        return ControlPacketBuilder.BuildRelayRegistration(
            sessionId: route.SessionId,
            sessionToken: route.SessionToken,
            role: "receiver");
    }

    private void SendRelayRegistrationFireAndForget()
    {
        var packet = BuildRelayRegistrationPacket();
        if (packet is null)
        {
            return;
        }

        _ = Task.Run(
            async () =>
            {
                try
                {
                    await SendRelayRegistrationPacketAsync(packet, CancellationToken.None);
                }
                catch
                {
                }
            });
    }

    private void UpdateStallDiagnostics(long nowTicks)
    {
        if (!_snapshot.Listening)
        {
            ResetStallLogState();
            return;
        }

        var playbackStatus = _snapshot.PlaybackStatus;
        var activelyPlaying =
            playbackStatus.StartsWith("Playing", StringComparison.OrdinalIgnoreCase) ||
            playbackStatus.StartsWith("Buffering", StringComparison.OrdinalIgnoreCase) ||
            playbackStatus.StartsWith("Opening", StringComparison.OrdinalIgnoreCase);
        if (!activelyPlaying)
        {
            ResetStallLogState();
            return;
        }

        var cinemaSmooth = _currentSessionConfig?.IsCinemaSmooth ?? false;
        var videoThresholdMs = cinemaSmooth ? 2400 : 1200;
        var decodeThresholdMs = cinemaSmooth ? 2400 : 1200;
        var presentThresholdMs = cinemaSmooth ? 2200 : 900;

        UpdateSingleStallLog(
            ref _videoStallLogged,
            label: "video",
            elapsedMs: (int)ElapsedSince(_lastVideoArrivalAtTicks, nowTicks).TotalMilliseconds,
            thresholdMs: videoThresholdMs,
            queueInfo: $"{_snapshot.StreamQueuedAccessUnits} AU / {_snapshot.StreamQueuedKilobytes} KB",
            extra: $"lastPacket={_snapshot.LastPacketType}, playback={playbackStatus}");
        UpdateSingleStallLog(
            ref _decodeStallLogged,
            label: "decode",
            elapsedMs: (int)ElapsedSince(_lastFrameDecodedAtTicks, nowTicks).TotalMilliseconds,
            thresholdMs: decodeThresholdMs,
            queueInfo: $"{_snapshot.StreamQueuedAccessUnits} AU / {_snapshot.StreamQueuedKilobytes} KB",
            extra: $"arrival={_snapshot.ArrivalDeltaMs} ms, playback={playbackStatus}");
        UpdateSingleStallLog(
            ref _presentStallLogged,
            label: "present",
            elapsedMs: (int)ElapsedSince(_lastFramePresentedAtTicks, nowTicks).TotalMilliseconds,
            thresholdMs: presentThresholdMs,
            queueInfo: $"{_snapshot.StreamQueuedAccessUnits} AU / {_snapshot.StreamQueuedKilobytes} KB",
            extra: $"decode={_snapshot.DecodeDeltaMs} ms, playback={playbackStatus}");
    }

    private void UpdateSingleStallLog(ref bool logged, string label, int elapsedMs, int thresholdMs, string queueInfo, string extra)
    {
        if (elapsedMs >= thresholdMs)
        {
            if (!logged)
            {
                ReceiverTrace.Log($"Stall detected: {label} idle for {elapsedMs} ms; queue={queueInfo}; {extra}");
                logged = true;
            }
        }
        else if (logged && elapsedMs <= Math.Max(120, thresholdMs / 2))
        {
            ReceiverTrace.Log($"Stall recovered: {label} idle back to {elapsedMs} ms");
            logged = false;
        }
    }

    private void ResetStallLogState()
    {
        _videoStallLogged = false;
        _decodeStallLogged = false;
        _presentStallLogged = false;
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

    private static string DescribeSessionConfigChange(SessionConfig? previousConfig, SessionConfig config)
    {
        if (previousConfig is null)
        {
            return $"{config.Codec} {config.Width}x{config.Height} @{config.Fps} bitrate={config.Bitrate} split={config.IsSplitStream} preset={config.Preset} mode={config.AdaptationMode}";
        }

        var changes = new List<string>();
        if (!string.Equals(previousConfig.Codec, config.Codec, StringComparison.OrdinalIgnoreCase))
        {
            changes.Add($"codec {previousConfig.Codec}->{config.Codec}");
        }
        if (!string.Equals(previousConfig.Transport, config.Transport, StringComparison.OrdinalIgnoreCase))
        {
            changes.Add($"transport {previousConfig.Transport ?? "-"}->{config.Transport ?? "-"}");
        }
        if (previousConfig.Width != config.Width || previousConfig.Height != config.Height)
        {
            changes.Add($"resolution {previousConfig.Width}x{previousConfig.Height}->{config.Width}x{config.Height}");
        }
        if (previousConfig.Fps != config.Fps)
        {
            changes.Add($"fps {previousConfig.Fps}->{config.Fps}");
        }
        if (previousConfig.Bitrate != config.Bitrate)
        {
            changes.Add($"bitrate {previousConfig.Bitrate}->{config.Bitrate}");
        }
        if (previousConfig.IsSplitStream != config.IsSplitStream)
        {
            changes.Add($"streamMode {(previousConfig.IsSplitStream ? "split" : "single")}->{(config.IsSplitStream ? "split" : "single")}");
        }
        if (!string.Equals(previousConfig.EnhancementCodec, config.EnhancementCodec, StringComparison.OrdinalIgnoreCase))
        {
            changes.Add($"enhancementCodec {previousConfig.EnhancementCodec ?? "-"}->{config.EnhancementCodec ?? "-"}");
        }
        if (previousConfig.EnhancementMaxWidth != config.EnhancementMaxWidth || previousConfig.EnhancementMaxHeight != config.EnhancementMaxHeight)
        {
            changes.Add($"enhancementMax {previousConfig.EnhancementMaxWidth}x{previousConfig.EnhancementMaxHeight}->{config.EnhancementMaxWidth}x{config.EnhancementMaxHeight}");
        }
        if (!string.Equals(previousConfig.Preset, config.Preset, StringComparison.OrdinalIgnoreCase))
        {
            changes.Add($"preset {previousConfig.Preset}->{config.Preset}");
        }
        if (!string.Equals(previousConfig.AdaptationMode, config.AdaptationMode, StringComparison.OrdinalIgnoreCase))
        {
            changes.Add($"mode {previousConfig.AdaptationMode}->{config.AdaptationMode}");
        }

        return changes.Count == 0 ? "no effective change" : string.Join(", ", changes);
    }

    private static string CodecLabel(string codecMime)
    {
        return codecMime.Contains("hevc", StringComparison.OrdinalIgnoreCase)
            ? "H.265 / HEVC"
            : "H.264 / AVC";
    }

    private static string PacketTypeLabel(byte type)
    {
        return type switch
        {
            TransportProtocol.TypeSessionConfig => "SessionConfig",
            TransportProtocol.TypeCodecConfig => "CodecConfig",
            TransportProtocol.TypeVideoFrame => "VideoFrame",
            TransportProtocol.TypeControl => "Control",
            TransportProtocol.TypeAudioConfig => "AudioConfig",
            TransportProtocol.TypeAudioFrame => "AudioFrame",
            TransportProtocol.TypeEnhancementConfig => "EnhancementConfig",
            TransportProtocol.TypeEnhancementFrame => "EnhancementFrame",
            TransportProtocol.TypeRoiMetadata => "RoiMetadata",
            _ => $"Unknown({type})",
        };
    }

    private IPlaybackController CreatePlaybackController(PlaybackBackendKind backend)
    {
        var playback = PlaybackControllerFactory.Create(backend, _playbackHost);
        playback.StatusChanged += status =>
        {
            if (!IsActivePlayback(playback))
            {
                return;
            }

            if (status.StartsWith("MF decode error:", StringComparison.OrdinalIgnoreCase) ||
                string.Equals(status, "Error", StringComparison.OrdinalIgnoreCase) ||
                status.Contains("error", StringComparison.OrdinalIgnoreCase))
            {
                if (ForceFallbackFromMediaFoundation(new InvalidOperationException(status)))
                {
                    return;
                }
            }

            lock (_sync)
            {
                var clearError =
                    string.Equals(status, "Playing", StringComparison.OrdinalIgnoreCase) ||
                    string.Equals(status, "Opening", StringComparison.OrdinalIgnoreCase) ||
                    string.Equals(status, "Stopped", StringComparison.OrdinalIgnoreCase) ||
                    string.Equals(status, "Idle", StringComparison.OrdinalIgnoreCase) ||
                    string.Equals(status, "Restarting playback", StringComparison.OrdinalIgnoreCase);
                _snapshot = _snapshot with
                {
                    PlaybackBackend = playback.BackendLabel,
                    PlaybackStatus = status,
                    LastPlaybackError = IsPlaybackErrorStatus(status)
                        ? status
                        : clearError ? "-" : _snapshot.LastPlaybackError,
                    LastBackendFailure = IsPlaybackErrorStatus(status)
                        ? status
                        : _snapshot.LastBackendFailure,
                    Status = _snapshot.Listening ? _snapshot.Status : status,
                };
            }
        };
        playback.StreamStatsChanged += stats =>
        {
            if (!IsActivePlayback(playback))
            {
                return;
            }

            lock (_sync)
            {
                _snapshot = _snapshot with
                {
                    StreamQueuedAccessUnits = stats.QueuedAccessUnits,
                    StreamQueuedKilobytes = stats.QueuedBytes / 1024,
                    StreamDroppedAccessUnits = stats.DroppedAccessUnits,
                    WaitingForKeyFrame = stats.WaitingForKeyFrame,
                    AdaptiveJitterMs = _adaptiveJitterMs,
                };
            }
        };
        playback.EnhancementStreamStatsChanged += stats =>
        {
            if (!IsActivePlayback(playback))
            {
                return;
            }

            lock (_sync)
            {
                _snapshot = _snapshot with
                {
                    EnhancementQueuedAccessUnits = stats.QueuedAccessUnits,
                    EnhancementQueuedKilobytes = stats.QueuedBytes / 1024,
                    EnhancementDroppedFrames = stats.DroppedAccessUnits,
                };
            }
        };
        playback.FrameDecoded += ticks =>
        {
            if (!IsActivePlayback(playback))
            {
                return;
            }

            lock (_sync)
            {
                _snapshot = _snapshot with
                {
                    DecodeDeltaMs = ComputeDeltaMs(ref _lastFrameDecodedAtTicks, ticks),
                };
            }
        };
        playback.FramePresented += ticks =>
        {
            if (!IsActivePlayback(playback))
            {
                return;
            }

            lock (_sync)
            {
                _lastPresentedBaseTicks = ticks;
                _lastPresentedBasePresentationTimeUs = playback.LastPresentedBasePresentationTimeUs;
                if (_lastPresentedBasePresentationTimeUs > 0)
                {
                    TryFinalizeLatencyPulseEstimateLocked(_lastPresentedBasePresentationTimeUs);
                }
                _snapshot = _snapshot with
                {
                    PresentDeltaMs = ComputeDeltaMs(ref _lastFramePresentedAtTicks, ticks),
                };
            }
        };
        return playback;
    }

    private string GetPlaybackBackendLabel()
    {
        return _playbackBackendLabel;
    }

    private TimeSpan GetAdaptiveJitterDelay()
    {
        var cinemaSmooth = _currentSessionConfig?.IsCinemaSmooth ?? false;
        return _transportMode == ReceiverTransportMode.Udp && (GetEffectiveUltraLowLatencyMode() || cinemaSmooth)
            ? TimeSpan.FromMilliseconds(GetEffectiveAdaptiveJitterMs())
            : TimeSpan.Zero;
    }

    private int GetEffectiveAdaptiveJitterMs()
    {
        return _manualAdaptiveJitterMs > 0 ? _manualAdaptiveJitterMs : _adaptiveJitterMs;
    }

    private int GetEffectiveCatchUpThresholdMs()
    {
        return _manualCatchUpThresholdMs > 0 ? _manualCatchUpThresholdMs : 20;
    }

    private int GetEffectiveFeedbackTickMs(bool cinemaSmooth)
    {
        if (_manualFeedbackTickMs > 0)
        {
            return _manualFeedbackTickMs;
        }

        return cinemaSmooth ? 120 : (_ultraLowLatencyMode ? 70 : (_aggressiveMode ? 100 : 200));
    }

    private int GetEffectiveForcedCatchUpCooldownMs()
    {
        return _manualKeyFrameCooldownMs > 0 ? _manualKeyFrameCooldownMs : 160;
    }

    private int GetEffectiveKeyFrameCooldownMs(bool cinemaSmooth, bool criticalPressure)
    {
        if (_manualKeyFrameCooldownMs > 0)
        {
            return _manualKeyFrameCooldownMs;
        }

        return cinemaSmooth
            ? (criticalPressure ? 700 : 1000)
            : criticalPressure
                ? (_ultraLowLatencyMode ? 120 : _aggressiveMode ? 150 : 220)
                : (_ultraLowLatencyMode ? 180 : _aggressiveMode ? 220 : 320);
    }

    private int GetEffectivePanicQueueThresholdAu(bool cinemaSmooth)
    {
        if (_manualPanicQueueAu > 0)
        {
            return _manualPanicQueueAu;
        }

        return _transportMode == ReceiverTransportMode.AdbTunnelTcp
            ? (cinemaSmooth ? 3 : (_aggressiveMode ? 1 : 2))
            : (cinemaSmooth ? 4 : (_aggressiveMode ? 2 : 3));
    }

    private int GetEffectiveHighDeltaMs(bool cinemaSmooth)
    {
        return _manualHighDeltaMs;
    }

    private int GetEffectiveCriticalDeltaMs(bool cinemaSmooth)
    {
        return _manualCriticalDeltaMs;
    }

    private int GetEffectiveStartupGraceMs(bool cinemaSmooth)
    {
        if (_manualStartupGraceMs > 0)
        {
            return _manualStartupGraceMs;
        }

        return cinemaSmooth ? 1800 : (_ultraLowLatencyMode ? 900 : 1200);
    }

    private int GetEffectiveDropBurstStep(bool cinemaSmooth, bool critical)
    {
        if (_manualDropBurstStep > 0)
        {
            return _manualDropBurstStep;
        }

        if (critical)
        {
            return cinemaSmooth ? 14 : (_ultraLowLatencyMode ? 8 : _aggressiveMode ? 5 : 3);
        }

        return 1;
    }

    private bool GetEffectiveUltraLowLatencyMode()
    {
        return _ultraLowLatencyMode && !(_currentSessionConfig?.IsCinemaSmooth ?? false);
    }

    private bool GetEffectiveAggressiveMode()
    {
        return _aggressiveMode && !(_currentSessionConfig?.IsCinemaSmooth ?? false);
    }

    private int ComputeAdaptiveJitterMs(
        bool cinemaSmooth,
        bool startupGrace,
        bool highCadenceSpike,
        bool criticalCadenceSpike,
        bool highDropBurst,
        bool criticalDropBurst,
        int queuedFrames,
        int arrivalDeltaMs,
        int decodeDeltaMs,
        int presentDeltaMs)
    {
        if (_transportMode != ReceiverTransportMode.Udp || !(_ultraLowLatencyMode || cinemaSmooth))
        {
            return 0;
        }

        if (!string.Equals(_snapshot.PlaybackStatus, "Playing", StringComparison.OrdinalIgnoreCase))
        {
            return 0;
        }

        if (cinemaSmooth)
        {
            if (startupGrace)
            {
                return 14;
            }

            if (criticalCadenceSpike || criticalDropBurst)
            {
                return 32;
            }

            if (highCadenceSpike || highDropBurst || arrivalDeltaMs >= 24 || decodeDeltaMs >= 24)
            {
                return 24;
            }

            if (queuedFrames > 0 || arrivalDeltaMs >= 16 || decodeDeltaMs >= 16 || presentDeltaMs >= 5)
            {
                return 18;
            }

            return 12;
        }

        if (startupGrace)
        {
            return 0;
        }

        if (criticalCadenceSpike || criticalDropBurst)
        {
            return 4;
        }

        if (highCadenceSpike || highDropBurst || arrivalDeltaMs >= 22 || decodeDeltaMs >= 22)
        {
            return 3;
        }

        if (queuedFrames > 0 || arrivalDeltaMs >= 15 || decodeDeltaMs >= 15 || presentDeltaMs >= 4)
        {
            return 2;
        }

        return 1;
    }

    private bool IsActivePlayback(IPlaybackController playback)
    {
        return ReferenceEquals(Volatile.Read(ref _activePlaybackRef), playback);
    }

    private static bool IsPlaybackErrorStatus(string status)
    {
        return status.Contains("error", StringComparison.OrdinalIgnoreCase);
    }

    private static int ComputeDeltaMs(ref long lastTicks, long nowTicks)
    {
        var delta = lastTicks == 0 || nowTicks <= lastTicks
            ? -1
            : (int)Math.Round((nowTicks - lastTicks) * 1000.0 / Stopwatch.Frequency);
        lastTicks = nowTicks;
        return delta;
    }

    private static void TryTerminateAdbShellProcess(Process? process)
    {
        if (process is null)
        {
            return;
        }

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
        finally
        {
            try
            {
                process.Dispose();
            }
            catch
            {
            }
        }
    }

    private void RunOnPlaybackHostThread(Action action)
    {
        if (_playbackHost.IsDisposed)
        {
            throw new ObjectDisposedException(nameof(_playbackHost));
        }

        if (_playbackHost.IsHandleCreated && _playbackHost.InvokeRequired)
        {
            _playbackHost.Invoke(action);
            return;
        }

        action();
    }

    private T RunOnPlaybackHostThread<T>(Func<T> func)
    {
        if (_playbackHost.IsDisposed)
        {
            throw new ObjectDisposedException(nameof(_playbackHost));
        }

        if (_playbackHost.IsHandleCreated && _playbackHost.InvokeRequired)
        {
            return (T)_playbackHost.Invoke(func)!;
        }

        return func();
    }
}
