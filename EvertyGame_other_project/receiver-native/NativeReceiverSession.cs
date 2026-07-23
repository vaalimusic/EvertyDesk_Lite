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
    private readonly SemaphoreSlim _controlSendGate = new(1, 1);
    private readonly WindowsPerformanceHints _performanceHints = new();

    private UdpClient? _udpClient;
    private TcpListener? _tcpListener;
    private TcpClient? _tcpClient;
    private NetworkStream? _tcpStream;
    private FrameReassembler? _reassembler;
    private CancellationTokenSource? _cts;
    private Task? _receiveTask;
    private Task? _feedbackTask;
    private IPEndPoint? _remoteEndpoint;
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
    private long _lastFeedbackDrops;
    private long _lastRequestKeyFrameAtTicks;
    private long _lastFeedbackAtTicks;
    private long _lastVideoArrivalAtTicks;
    private long _lastFrameDecodedAtTicks;
    private long _lastFramePresentedAtTicks;
    private long _sessionReadyAtTicks;
    private long _lastForcedCatchUpAtTicks;
    private int _adaptiveJitterMs;

    public NativeReceiverSession(Control playbackHost)
    {
        _playbackHost = playbackHost;
        _playback = CreatePlaybackController(_playbackBackend);
    }

    public ReceiverSessionSnapshot GetSnapshot()
    {
        lock (_sync)
        {
            return _snapshot;
        }
    }

    public void Start(int port, ReceiverTransportMode transportMode, HardwareDecodeMode decodeMode, bool aggressiveMode)
    {
        Stop();
        _performanceHints.Enable();

        _transportMode = transportMode;
        _decodeMode = decodeMode;
        _aggressiveMode = aggressiveMode;
        _listeningPort = port;
        lock (_playbackSync)
        {
            _playback.UpdateHardwareDecodeMode(decodeMode);
            _playback.UpdateAggressiveMode(aggressiveMode);
            _playback.UpdateUltraLowLatencyMode(_ultraLowLatencyMode);
            _playback.UpdateAdaptiveJitterBuffer(GetAdaptiveJitterDelay());
        }

        UdpClient? udpClient = null;
        TcpListener? tcpListener = null;
        if (transportMode == ReceiverTransportMode.Udp)
        {
            udpClient = new UdpClient(port);
            udpClient.Client.ReceiveBufferSize = 512 * 1024;
            udpClient.Client.SendBufferSize = 64 * 1024;
        }
        else
        {
            tcpListener = new TcpListener(IPAddress.Any, port);
            tcpListener.Server.ReceiveBufferSize = 512 * 1024;
            tcpListener.Server.SendBufferSize = 64 * 1024;
            tcpListener.Start(1);
        }

        var reassembler = new FrameReassembler(
            onSessionConfig: HandleSessionConfig,
            onBaseAccessUnitReady: HandleAccessUnitReady,
            onEnhancementAccessUnitReady: HandleEnhancementAccessUnitReady,
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
        _reassembler = reassembler;
        _cts = cts;
        lock (_sync)
        {
            _snapshot = new ReceiverSessionSnapshot
            {
                Listening = true,
                Status = transportMode.BuildWaitingStatus(port),
                TransportMode = transportMode.ToUiLabel(),
                PlaybackBackend = GetPlaybackBackendLabel(),
                PlaybackStatus = "Idle",
                DecodeMode = decodeMode.ToUiLabel(),
                UltraLowLatencyMode = _ultraLowLatencyMode,
                AdaptiveJitterMs = _adaptiveJitterMs,
                SystemHintsEnabled = true,
                LastBackendFailure = "-",
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
            _sessionReadyAtTicks = 0;
            _lastForcedCatchUpAtTicks = 0;
            _currentSessionConfig = null;
        }

        _receiveTask = Task.Run(() => transportMode == ReceiverTransportMode.Udp
            ? ReceiveUdpLoopAsync(cts.Token)
            : ReceiveTcpLoopAsync(cts.Token));
        _feedbackTask = Task.Run(() => FeedbackLoopAsync(cts.Token));
    }

    public void UpdateHardwareDecodeMode(HardwareDecodeMode mode)
    {
        _decodeMode = mode;
        lock (_playbackSync)
        {
            _playback.UpdateHardwareDecodeMode(mode);
        }
        lock (_sync)
        {
            _snapshot = _snapshot with { DecodeMode = mode.ToUiLabel() };
        }
        RequestKeyFrame();
    }

    public void UpdatePlaybackBackend(PlaybackBackendKind backend)
    {
        IPlaybackController? previousPlayback = null;
        IPlaybackController? newPlayback = null;
        SessionConfig? sessionConfig;
        bool listening;

        lock (_playbackSync)
        {
            if (_playbackBackend == backend)
            {
                return;
            }

            previousPlayback = _playback;
            newPlayback = RunOnPlaybackHostThread(() => CreatePlaybackController(backend));
            newPlayback.UpdateHardwareDecodeMode(_decodeMode);
            newPlayback.UpdateAggressiveMode(_aggressiveMode);
            newPlayback.UpdateUltraLowLatencyMode(_ultraLowLatencyMode);
            newPlayback.UpdateAdaptiveJitterBuffer(GetAdaptiveJitterDelay());

            sessionConfig = _currentSessionConfig;
            if (sessionConfig is not null)
            {
                newPlayback.ApplySessionConfig(sessionConfig);
                newPlayback.WaitForKeyFrame();
            }

            _playback = newPlayback;
            _playbackBackend = backend;
            listening = GetSnapshot().Listening;
        }

        previousPlayback.Dispose();

        lock (_sync)
        {
            _snapshot = _snapshot with
            {
                PlaybackBackend = newPlayback.BackendLabel,
                PlaybackStatus = listening ? "Switching backend" : "Idle",
                LastPlaybackError = "-",
                LastBackendFailure = "-",
                AdaptiveJitterMs = _adaptiveJitterMs,
            };
        }

        if (listening)
        {
            RequestKeyFrame();
        }
    }

    public void UpdateAggressiveMode(bool enabled)
    {
        _aggressiveMode = enabled;
        lock (_playbackSync)
        {
            _playback.UpdateAggressiveMode(enabled);
        }
        RequestKeyFrame();
    }

    public void UpdateUltraLowLatencyMode(bool enabled)
    {
        _ultraLowLatencyMode = enabled;
        lock (_playbackSync)
        {
            _playback.UpdateUltraLowLatencyMode(enabled);
            _playback.UpdateAdaptiveJitterBuffer(GetAdaptiveJitterDelay());
        }
        lock (_sync)
        {
            _snapshot = _snapshot with
            {
                UltraLowLatencyMode = enabled,
                AdaptiveJitterMs = enabled && _transportMode == ReceiverTransportMode.Udp ? _adaptiveJitterMs : 0,
            };
        }
        RequestKeyFrame();
    }

    public void Stop()
    {
        var cts = _cts;
        var udpClient = _udpClient;
        var tcpStream = _tcpStream;
        var tcpClient = _tcpClient;
        var tcpListener = _tcpListener;
        var receiveTask = _receiveTask;
        var feedbackTask = _feedbackTask;

        _cts = null;
        _udpClient = null;
        _tcpStream = null;
        _tcpClient = null;
        _tcpListener = null;
        _reassembler = null;
        _receiveTask = null;
        _feedbackTask = null;

        if (cts is not null)
        {
            cts.Cancel();
        }

        udpClient?.Dispose();
        tcpStream?.Dispose();
        tcpClient?.Dispose();
        tcpListener?.Stop();

        try
        {
            receiveTask?.Wait(400);
            feedbackTask?.Wait(400);
        }
        catch
        {
        }

        lock (_playbackSync)
        {
            _currentSessionConfig = null;
            _playback.WaitForKeyFrame();
        }

        lock (_sync)
        {
            _snapshot = new ReceiverSessionSnapshot
            {
                Status = "Idle",
                TransportMode = _transportMode.ToUiLabel(),
                PlaybackBackend = GetPlaybackBackendLabel(),
                PlaybackStatus = "Stopped",
                DecodeMode = _decodeMode.ToUiLabel(),
                UltraLowLatencyMode = _ultraLowLatencyMode,
                SystemHintsEnabled = false,
            };
            _remoteEndpoint = null;
            _inputFrameTicks.Clear();
            _enhancementFrameTicks.Clear();
            _lastVideoArrivalAtTicks = 0;
            _lastFrameDecodedAtTicks = 0;
            _lastFramePresentedAtTicks = 0;
            _sessionReadyAtTicks = 0;
            _lastForcedCatchUpAtTicks = 0;
        }

        _performanceHints.Disable();
    }

    public void Dispose()
    {
        Stop();
        lock (_playbackSync)
        {
            _playback.Dispose();
        }
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
                    lock (_playbackSync)
                    {
                        _playback.WaitForKeyFrame();
                    }
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
        }

        if (packet.Type is TransportProtocol.TypeAudioConfig or TransportProtocol.TypeAudioFrame)
        {
            return;
        }

        _reassembler?.OnPacket(packet);
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
            int? desiredJitterMs = null;

            lock (_sync)
            {
                var transportReady = _transportMode == ReceiverTransportMode.Udp
                    ? _udpClient is not null && _remoteEndpoint is not null
                    : _tcpStream is not null && _tcpClient is not null;
                if (!transportReady || !_snapshot.Listening)
                {
                    continue;
                }

                var nowTicks = Stopwatch.GetTimestamp();
                var feedbackCooldown = TimeSpan.FromMilliseconds(_ultraLowLatencyMode ? 70 : (_aggressiveMode ? 100 : 200));
                if (ElapsedSince(_lastFeedbackAtTicks, nowTicks) < feedbackCooldown)
                {
                    continue;
                }

                var startupGrace =
                    _sessionReadyAtTicks != 0 &&
                    ElapsedSince(_sessionReadyAtTicks, nowTicks) < TimeSpan.FromMilliseconds(_ultraLowLatencyMode ? 900 : 1200) &&
                    _snapshot.FramesAssembled < (_ultraLowLatencyMode ? 18 : 24);

                var playbackBuffering =
                    _snapshot.PlaybackStatus.StartsWith("Buffering", StringComparison.OrdinalIgnoreCase) ||
                    _snapshot.PlaybackStatus.StartsWith("Opening", StringComparison.OrdinalIgnoreCase);
                var playbackRecoveryPressure = playbackBuffering || _snapshot.WaitingForKeyFrame;
                var queuedFrames = _snapshot.StreamQueuedAccessUnits;
                var hasLanBacklog = _transportMode == ReceiverTransportMode.AdbTunnelTcp
                    ? queuedFrames > 0
                    : queuedFrames > 1;
                var transportBacklogCritical =
                    _transportMode == ReceiverTransportMode.AdbTunnelTcp &&
                    queuedFrames > 0;
                var criticalAssemblyDelayMs = _transportMode == ReceiverTransportMode.AdbTunnelTcp
                    ? (_ultraLowLatencyMode ? 18 : 28)
                    : (_ultraLowLatencyMode ? 10 : 18);
                var highAssemblyDelayMs = _transportMode == ReceiverTransportMode.AdbTunnelTcp
                    ? (_ultraLowLatencyMode ? 10 : 18)
                    : (_ultraLowLatencyMode ? 6 : 12);
                var highCadenceDeltaMs = _transportMode == ReceiverTransportMode.AdbTunnelTcp
                    ? (_ultraLowLatencyMode ? 28 : 34)
                    : (_ultraLowLatencyMode ? 22 : 28);
                var criticalCadenceDeltaMs = _transportMode == ReceiverTransportMode.AdbTunnelTcp
                    ? (_ultraLowLatencyMode ? 42 : 52)
                    : (_ultraLowLatencyMode ? 34 : 42);
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
                    _lastFeedbackDrops + (_ultraLowLatencyMode ? 8 : _aggressiveMode ? 5 : 3);
                var highDropBurst = !startupGrace && _snapshot.StreamDroppedAccessUnits > _lastFeedbackDrops;
                var criticalLowInputFps =
                    _snapshot.TargetFps >= 24 &&
                    _snapshot.InputFpsProxy > 0 &&
                    !startupGrace &&
                    (hasLanBacklog || playbackRecoveryPressure || highDropBurst) &&
                    _snapshot.InputFpsProxy <
                    (_aggressiveMode ? (int)(_snapshot.TargetFps * 0.45f) : (int)(_snapshot.TargetFps * 0.40f));
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
                    (_aggressiveMode ? (int)(_snapshot.TargetFps * 0.80f) : (int)(_snapshot.TargetFps * 0.70f));
                var criticalPressure =
                    transportBacklogCritical ||
                    _snapshot.AssemblyDelayMs >= criticalAssemblyDelayMs ||
                    criticalCadenceSpike ||
                    queuedFrames > (_transportMode == ReceiverTransportMode.AdbTunnelTcp ? (_aggressiveMode ? 1 : 2) : (_aggressiveMode ? 2 : 3)) ||
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
                    presentDeltaMs: _snapshot.PresentDeltaMs);
                _lastFeedbackAtTicks = nowTicks;

                var networkStallCatchUp =
                    _transportMode == ReceiverTransportMode.Udp &&
                    _ultraLowLatencyMode &&
                    !startupGrace &&
                    _snapshot.ArrivalDeltaMs >= 20 &&
                    _snapshot.ArrivalDeltaMs >= 0;
                if (networkStallCatchUp &&
                    ElapsedSince(_lastForcedCatchUpAtTicks, nowTicks) >= TimeSpan.FromMilliseconds(160))
                {
                    lock (_playbackSync)
                    {
                        _playback.WaitForKeyFrame();
                    }
                    requestKeyFrame = ControlPacketBuilder.BuildRequestKeyFrame();
                    _lastRequestKeyFrameAtTicks = nowTicks;
                    _lastForcedCatchUpAtTicks = nowTicks;
                }

                if (requestKeyFrame is null &&
                    highPressure &&
                    ElapsedSince(_lastRequestKeyFrameAtTicks, nowTicks) >= TimeSpan.FromMilliseconds(
                        criticalPressure
                            ? (_ultraLowLatencyMode ? 120 : _aggressiveMode ? 150 : 220)
                            : (_ultraLowLatencyMode ? 180 : _aggressiveMode ? 220 : 320)))
                {
                    requestKeyFrame = ControlPacketBuilder.BuildRequestKeyFrame();
                    _lastRequestKeyFrameAtTicks = nowTicks;
                }

                _lastFeedbackDrops = _snapshot.StreamDroppedAccessUnits;
            }

            if (desiredJitterMs.HasValue)
            {
                lock (_playbackSync)
                {
                    _playback.UpdateAdaptiveJitterBuffer(TimeSpan.FromMilliseconds(desiredJitterMs.Value));
                }
            }

            try
            {
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

    private void HandleSessionConfig(SessionConfig config)
    {
        SessionConfig? previousConfig;
        lock (_sync)
        {
            previousConfig = _currentSessionConfig;
        }

        var requiresPlaybackReconfigure =
            previousConfig is null ||
            !string.Equals(previousConfig.Codec, config.Codec, StringComparison.OrdinalIgnoreCase) ||
            !string.Equals(previousConfig.Transport, config.Transport, StringComparison.OrdinalIgnoreCase) ||
            previousConfig.Width != config.Width ||
            previousConfig.Height != config.Height ||
            previousConfig.Fps != config.Fps;

        if (!requiresPlaybackReconfigure)
        {
            lock (_sync)
            {
                _currentSessionConfig = config;
                _snapshot = _snapshot with
                {
                    Status = $"Receiving {CodecLabel(config.Codec)} {config.ResolutionLabel}",
                    PlaybackBackend = GetPlaybackBackendLabel(),
                    StreamMode = config.StreamMode,
                    Codec = CodecLabel(config.Codec),
                    Preset = config.Preset,
                    Resolution = config.ResolutionLabel,
                    TargetFps = config.Fps,
                    BitrateMbps = config.Bitrate / 1_000_000.0,
                    AdaptiveJitterMs = _adaptiveJitterMs,
                };
            }
            return;
        }

        Exception? applyError = null;
        lock (_playbackSync)
        {
            _currentSessionConfig = config;
            try
            {
                _playback.ApplySessionConfig(config);
            }
            catch (Exception ex)
            {
                applyError = ex;
            }
        }

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

        lock (_sync)
        {
            _snapshot = _snapshot with
            {
                Status = $"Receiving {CodecLabel(config.Codec)} {config.ResolutionLabel}",
                PlaybackBackend = GetPlaybackBackendLabel(),
                StreamMode = config.StreamMode,
                Codec = CodecLabel(config.Codec),
                Preset = config.Preset,
                Resolution = config.ResolutionLabel,
                TargetFps = config.Fps,
                BitrateMbps = config.Bitrate / 1_000_000.0,
                AdaptiveJitterMs = _adaptiveJitterMs,
            };
            _sessionReadyAtTicks = Stopwatch.GetTimestamp();
            _inputFrameTicks.Clear();
            _enhancementFrameTicks.Clear();
            _lastVideoArrivalAtTicks = 0;
            _lastFrameDecodedAtTicks = 0;
            _lastFramePresentedAtTicks = 0;
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
            fallbackPlayback.UpdateAggressiveMode(_aggressiveMode);
            fallbackPlayback.UpdateUltraLowLatencyMode(_ultraLowLatencyMode);
            fallbackPlayback.UpdateAdaptiveJitterBuffer(GetAdaptiveJitterDelay());
            fallbackPlayback.ApplySessionConfig(config);
            fallbackPlayback.WaitForKeyFrame();

            lock (_playbackSync)
            {
                if (_playbackBackend != PlaybackBackendKind.MediaFoundationDirect3D11)
                {
                    fallbackPlayback.Dispose();
                    return false;
                }

                previousPlayback = _playback;
                _playback = fallbackPlayback;
                _playbackBackend = PlaybackBackendKind.LibVlcHwndDirect3D11;
                _currentSessionConfig = config;
                fallbackPlayback = null;
            }

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
        SessionConfig? config;
        lock (_playbackSync)
        {
            config = _currentSessionConfig;
        }

        return config is not null && TryFallbackFromMediaFoundation(config, originalError);
    }

    private bool ForceFallbackFromMediaFoundation(Exception originalError)
    {
        PlaybackBackendKind currentBackend;
        SessionConfig? config;
        lock (_playbackSync)
        {
            currentBackend = _playbackBackend;
            config = _currentSessionConfig;
        }

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

    private void HandleAccessUnitReady(byte[] bytes, bool isKeyFrame, int assemblyDelayMs, long presentationTimeUs)
    {
        try
        {
            lock (_playbackSync)
            {
                _playback.EnqueueAccessUnit(bytes, isKeyFrame, presentationTimeUs);
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

    private void HandleEnhancementAccessUnitReady(EnhancementAccessUnit accessUnit)
    {
        try
        {
            lock (_playbackSync)
            {
                _playback.EnqueueEnhancementAccessUnit(
                    accessUnit.Bytes,
                    accessUnit.IsKeyFrame,
                    accessUnit.PresentationTimeUs,
                    accessUnit.Metadata);
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
                    endpoint = _remoteEndpoint;
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

    private void RequestKeyFrame()
    {
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

    private static TimeSpan ElapsedSince(long thenTicks, long nowTicks)
    {
        if (thenTicks == 0 || nowTicks <= thenTicks)
        {
            return TimeSpan.Zero;
        }

        var seconds = (nowTicks - thenTicks) / (double)Stopwatch.Frequency;
        return TimeSpan.FromSeconds(seconds);
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
        lock (_playbackSync)
        {
            return _playback.BackendLabel;
        }
    }

    private TimeSpan GetAdaptiveJitterDelay()
    {
        return _transportMode == ReceiverTransportMode.Udp && _ultraLowLatencyMode
            ? TimeSpan.FromMilliseconds(_adaptiveJitterMs)
            : TimeSpan.Zero;
    }

    private int ComputeAdaptiveJitterMs(
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
        if (_transportMode != ReceiverTransportMode.Udp || !_ultraLowLatencyMode)
        {
            return 0;
        }

        if (startupGrace || !string.Equals(_snapshot.PlaybackStatus, "Playing", StringComparison.OrdinalIgnoreCase))
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
        lock (_playbackSync)
        {
            return ReferenceEquals(_playback, playback);
        }
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
