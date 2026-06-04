using System;
using System.Collections.Generic;
using System.Collections.ObjectModel;
using System.ComponentModel;
using System.Linq;
using System.Net;
using System.Runtime.CompilerServices;
using System.Threading.Tasks;
using Avalonia.Threading;
using CPC = Everty.ControlPlane.Contracts;
using ReceiverNative;

namespace Everty.Desktop.Avalonia;

internal sealed class MainWindowViewModel : INotifyPropertyChanged, IDisposable, IDesktopHostViewModel, IDesktopClientViewModel, IPlaybackSurface
{
    private readonly IHostControlPlaneAgent _hostAgent = new HostControlPlaneAgentAdapter();
    private readonly IControlPlaneSessionService _controlPlaneClient = new ControlPlaneSessionService();
    private readonly IWindowsSenderRuntime _senderSession = new WindowsSenderRuntimeAdapter();
    private readonly DesktopClientRuntime _clientRuntime = new();
    private readonly DesktopUiPreferences _uiPreferences = DesktopUiPreferences.Load();
    private readonly object _sync = new();
    private readonly DispatcherTimer _telemetryTimer;
    private bool _disposed;

    private string _controlPlaneUrl = "http://46.45.217.19:5180";
    private string _hostStatus = "Idle";
    private string _hostDetail = "Host not running";
    private string _hostSnapshot = "-";
    private string _hostCode = "-";
    private string _clientStatus = "Idle";
    private string _clientDetail = "Client not connected";
    private string _clientSession = "-";
    private string _clientRoute = "-";
    private string _clientEndpoint = "-";
    private string _clientCodec = "-";
    private string _clientSnapshot = "-";
    private string _selectedClientPreset = WindowsSenderPreset.Game.ToSpec().UiLabel;
    private string _selectedPreset = WindowsSenderPreset.Game.ToSpec().UiLabel;
    private string _selectedCodec = WindowsVideoCodec.H265Hevc.ToUiLabel();
    private string _selectedEncoder = "Auto";
    private string _captureTarget = string.Empty;
    private string _selectedHostCode = string.Empty;
    private CPC.DesktopControlPlaneHostSummary? _selectedHost;
    private bool _hostRunning;
    private bool _diagnosticsVisible;
    private bool _advancedVisible;
    private bool _hostAdaptiveStreamingEnabled = true;
    private int _selectedTabIndex;
    private bool _clientAutoRestoreAttempted;
    private string _hostProfileWidth = string.Empty;
    private string _hostProfileHeight = string.Empty;
    private string _hostProfileFps = string.Empty;
    private string _hostProfileBitrateMbps = string.Empty;
    private const int DesktopClientListenPort = 5001;

    private sealed record HostProfileOverride(int Width, int Height, int Fps, int BitrateBps);

    public event PropertyChangedEventHandler? PropertyChanged;

    public MainWindowViewModel()
    {
        HostCaptureTargets = new ObservableCollection<string>(
            _senderSession.GetCaptureTargets().Select(static target => target.DeviceName));
        if (HostCaptureTargets.Count > 0)
        {
            _captureTarget = HostCaptureTargets[0];
        }

        HostPresets = new ObservableCollection<string>(Enum.GetNames<WindowsSenderPreset>());
        ClientPresets = new ObservableCollection<string>(new[] { "Low Latency", "Balanced", "Quality" });
        HostCodecs = new ObservableCollection<string>(
            Enum.GetValues<WindowsVideoCodec>().Select(static codec => codec.ToUiLabel()));
        HostEncoders = new ObservableCollection<string>(
            new[]
            {
                WindowsSenderEncoderBackend.Auto.ToUiLabel(),
                WindowsSenderEncoderBackend.NvidiaNvencNative.ToUiLabel(),
                WindowsSenderEncoderBackend.NvidiaNvenc.ToUiLabel(),
                WindowsSenderEncoderBackend.IntelQuickSync.ToUiLabel(),
                WindowsSenderEncoderBackend.MediaFoundation.ToUiLabel(),
                WindowsSenderEncoderBackend.FfmpegSoftware.ToUiLabel(),
            });

        _diagnosticsVisible = _uiPreferences.DiagnosticsVisible;
        _advancedVisible = _uiPreferences.AdvancedVisible;
        if (!string.IsNullOrWhiteSpace(_uiPreferences.HostPreset))
        {
            _selectedPreset = _uiPreferences.HostPreset;
        }

        if (!string.IsNullOrWhiteSpace(_uiPreferences.ClientPreset))
        {
            _selectedClientPreset = _uiPreferences.ClientPreset;
        }

        if (!string.IsNullOrWhiteSpace(_uiPreferences.HostCodec))
        {
            _selectedCodec = _uiPreferences.HostCodec;
        }

        if (!string.IsNullOrWhiteSpace(_uiPreferences.HostEncoder))
        {
            _selectedEncoder = _uiPreferences.HostEncoder;
        }

        if (!string.IsNullOrWhiteSpace(_uiPreferences.CaptureTarget))
        {
            _captureTarget = _uiPreferences.CaptureTarget;
        }

        if (!string.IsNullOrWhiteSpace(_uiPreferences.SelectedHostCode))
        {
            _selectedHostCode = _uiPreferences.SelectedHostCode;
        }

        if (!string.IsNullOrWhiteSpace(_uiPreferences.ControlPlaneUrl))
        {
            _controlPlaneUrl = _uiPreferences.ControlPlaneUrl;
        }

        _selectedTabIndex = Math.Clamp(_uiPreferences.SelectedTabIndex, 0, 1);
        _hostAdaptiveStreamingEnabled = _uiPreferences.HostAdaptiveStreamingEnabled;
        LoadHostProfileFields();

        _hostAgent.SnapshotChanged += HandleHostSnapshotChanged;
        _telemetryTimer = new DispatcherTimer
        {
            Interval = TimeSpan.FromMilliseconds(500),
        };
        _telemetryTimer.Tick += (_, _) =>
        {
            RefreshHostTelemetry();
            RefreshClientRuntimeTelemetry();
        };
        _telemetryTimer.Start();

        Dispatcher.UIThread.Post(
            async () =>
            {
                if (!_clientAutoRestoreAttempted && CanRestoreClientSession)
                {
                    _clientAutoRestoreAttempted = true;
                    await RestoreManagedSessionAsync();
                }
            },
            DispatcherPriority.Background);
    }

    public ObservableCollection<string> HostCaptureTargets { get; }
    public ObservableCollection<string> HostPresets { get; }
    public ObservableCollection<string> ClientPresets { get; }
    public ObservableCollection<string> HostCodecs { get; }
    public ObservableCollection<string> HostEncoders { get; }
    public ObservableCollection<CPC.DesktopControlPlaneHostSummary> Hosts { get; } = new();

    public string ControlPlaneUrl
    {
        get => _controlPlaneUrl;
        set
        {
            if (SetProperty(ref _controlPlaneUrl, value))
            {
                _uiPreferences.ControlPlaneUrl = value;
                _uiPreferences.Save();
            }
        }
    }

    public string HostStatus
    {
        get => _hostStatus;
        private set => SetProperty(ref _hostStatus, value);
    }

    public string HostDetail
    {
        get => _hostDetail;
        private set => SetProperty(ref _hostDetail, value);
    }

    public string HostSnapshot
    {
        get => _hostSnapshot;
        private set => SetProperty(ref _hostSnapshot, value);
    }

    public string HostCode
    {
        get => _hostCode;
        private set => SetProperty(ref _hostCode, value);
    }

    public string ClientStatus
    {
        get => _clientStatus;
        private set => SetProperty(ref _clientStatus, value);
    }

    public string ClientDetail
    {
        get => _clientDetail;
        private set => SetProperty(ref _clientDetail, value);
    }

    public string ClientSession
    {
        get => _clientSession;
        private set => SetProperty(ref _clientSession, value);
    }

    public string ClientRoute
    {
        get => _clientRoute;
        private set => SetProperty(ref _clientRoute, value);
    }

    public string ClientEndpoint
    {
        get => _clientEndpoint;
        private set => SetProperty(ref _clientEndpoint, value);
    }

    public string ClientCodec
    {
        get => _clientCodec;
        private set => SetProperty(ref _clientCodec, value);
    }

    public string ClientSnapshot
    {
        get => _clientSnapshot;
        private set => SetProperty(ref _clientSnapshot, value);
    }

    public string SelectedPreset
    {
        get => _selectedPreset;
        set
        {
            if (SetProperty(ref _selectedPreset, value))
            {
                _uiPreferences.HostPreset = value;
                _uiPreferences.Save();
                LoadHostProfileFields();
            }
        }
    }

    public string HostProfileWidth
    {
        get => _hostProfileWidth;
        set
        {
            var sanitized = new string((value ?? string.Empty).Where(char.IsDigit).Take(4).ToArray());
            if (SetProperty(ref _hostProfileWidth, sanitized))
            {
                SaveHostProfileFields();
            }
        }
    }

    public string HostProfileHeight
    {
        get => _hostProfileHeight;
        set
        {
            var sanitized = new string((value ?? string.Empty).Where(char.IsDigit).Take(4).ToArray());
            if (SetProperty(ref _hostProfileHeight, sanitized))
            {
                SaveHostProfileFields();
            }
        }
    }

    public string HostProfileFps
    {
        get => _hostProfileFps;
        set
        {
            var sanitized = new string((value ?? string.Empty).Where(char.IsDigit).Take(3).ToArray());
            if (SetProperty(ref _hostProfileFps, sanitized))
            {
                SaveHostProfileFields();
            }
        }
    }

    public string HostProfileBitrateMbps
    {
        get => _hostProfileBitrateMbps;
        set
        {
            var sanitized = new string((value ?? string.Empty).Where(static ch => char.IsDigit(ch) || ch is '.' or ',').Take(6).ToArray());
            if (SetProperty(ref _hostProfileBitrateMbps, sanitized))
            {
                SaveHostProfileFields();
            }
        }
    }

    public string SelectedPresetSummary
    {
        get
        {
            var spec = GetSelectedHostPresetSpec();
            return $"{spec.ProtocolPreset} · {spec.TargetWidth}x{spec.TargetHeight} @ {spec.TargetFps} fps · {(spec.TargetBitrateBps / 1_000_000.0):0.0} Mbps";
        }
    }

    public string SelectedClientPreset
    {
        get => _selectedClientPreset;
        set
        {
            if (SetProperty(ref _selectedClientPreset, value))
            {
                _uiPreferences.ClientPreset = value;
                _uiPreferences.Save();
                RefreshClientTelemetry();
            }
        }
    }

    public string SelectedCodec
    {
        get => _selectedCodec;
        set
        {
            if (SetProperty(ref _selectedCodec, value))
            {
                _uiPreferences.HostCodec = value;
                _uiPreferences.Save();
            }
        }
    }

    public string SelectedEncoder
    {
        get => _selectedEncoder;
        set
        {
            if (SetProperty(ref _selectedEncoder, value))
            {
                _uiPreferences.HostEncoder = value;
                _uiPreferences.Save();
            }
        }
    }

    public string CaptureTarget
    {
        get => _captureTarget;
        set
        {
            if (SetProperty(ref _captureTarget, value))
            {
                _uiPreferences.CaptureTarget = value;
                _uiPreferences.Save();
            }
        }
    }

    public bool HostAdaptiveStreamingEnabled
    {
        get => _hostAdaptiveStreamingEnabled;
        set
        {
            if (SetProperty(ref _hostAdaptiveStreamingEnabled, value))
            {
                _uiPreferences.HostAdaptiveStreamingEnabled = value;
                _uiPreferences.Save();
                PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(DiagnosticsText)));
            }
        }
    }

    public string SelectedHostCode
    {
        get => _selectedHostCode;
        set
        {
            if (SetProperty(ref _selectedHostCode, value))
            {
                _uiPreferences.SelectedHostCode = value;
                _uiPreferences.Save();
                TrySelectHostByCode();
                PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(SelectedHostSummary)));
            }
        }
    }

    public CPC.DesktopControlPlaneHostSummary? SelectedHost
    {
        get => _selectedHost;
        set
        {
            if (SetProperty(ref _selectedHost, value) && value is not null)
            {
                SelectedHostCode = value.HostCode;
                PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(SelectedHostSummary)));
            }
        }
    }

    public bool HostRunning
    {
        get => _hostRunning;
        private set => SetProperty(ref _hostRunning, value);
    }

    public bool DiagnosticsVisible
    {
        get => _diagnosticsVisible;
        private set => SetProperty(ref _diagnosticsVisible, value);
    }

    public bool AdvancedVisible
    {
        get => _advancedVisible;
        private set => SetProperty(ref _advancedVisible, value);
    }

    public int SelectedTabIndex
    {
        get => _selectedTabIndex;
        set
        {
            if (SetProperty(ref _selectedTabIndex, Math.Clamp(value, 0, 1)))
            {
                _uiPreferences.SelectedTabIndex = _selectedTabIndex;
                _uiPreferences.Save();
            }
        }
    }

    public string DiagnosticsText =>
        string.Join(Environment.NewLine, new[]
        {
            $"Host status    : {HostStatus}",
            $"Host code      : {HostCode}",
            $"Host detail    : {HostDetail}",
            $"Host snapshot  : {HostSnapshot}",
            $"Client status  : {ClientStatus}",
            $"Client detail  : {ClientDetail}",
            $"Client route   : {ClientRoute}",
            $"Client endpoint: {ClientEndpoint}",
            $"Client codec   : {ClientCodec}",
            $"Client snapshot: {ClientSnapshot}",
        });

    public string ClientPresetSummary => SelectedClientPreset switch
    {
        "Low Latency" => "720p60, adaptive on, AVC-first for faster startup and tighter latency.",
        "Quality" => "1080p60, adaptive off, AV1/HEVC-first for image quality.",
        _ => "1600x900@60, adaptive on, HEVC-first balanced profile.",
    };

    public string SelectedHostSummary => SelectedHost is null
        ? "No host selected. Refresh hosts or enter a short code."
        : $"{SelectedHost.DisplayName} [{SelectedHost.HostCode}] · {SelectedHost.Region} · {(SelectedHost.Online ? "Online" : "Offline")} · {(string.IsNullOrWhiteSpace(SelectedHost.ActiveSessionId) ? "Available" : "Busy")}";

    public bool CanRestoreClientSession => _controlPlaneClient.GetManagedSessionState(ControlPlaneUrl) is not null;

    public bool CanOpenClientPlaybackWindow => _clientRuntime.HasActivePlaybackWindow;

    public bool IsClientPlaybackWindowVisible => _clientRuntime.IsPlaybackWindowVisible;

    public string PlaybackTitle => HostRunning || !string.Equals(ClientSession, "-", StringComparison.Ordinal)
        ? "Playback surface"
        : "Idle surface";

    public string PlaybackDescription => HostRunning
        ? string.Join(Environment.NewLine, new[]
        {
            $"Host ready: {HostDetail}",
            $"Encoder    : {_senderSession.GetSnapshot().EncoderPath}",
            $"Codec      : {_senderSession.GetSnapshot().Codec}",
            $"Capture    : {_senderSession.GetSnapshot().Resolution}",
        })
        : !string.Equals(ClientSession, "-", StringComparison.Ordinal)
            ? string.Join(Environment.NewLine, new[]
            {
                $"Client session: {ClientSession}",
                $"Route         : {ClientRoute}",
                $"Endpoint      : {ClientEndpoint}",
                $"Codec         : {ClientCodec}",
                $"Health        : {ClientStatus}",
                $"Playback      : {_clientRuntime.GetSnapshot().PlaybackStatus}",
            })
            : "No active session";

    string IPlaybackSurface.Title => PlaybackTitle;
    string IPlaybackSurface.Description => PlaybackDescription;

    public async Task StartHostAsync()
    {
        if (!OperatingSystem.IsWindows())
        {
            HostStatus = "Windows-only backend";
            HostDetail = "Host runtime requires Windows APIs.";
            return;
        }

        var probe = _senderSession.GetCapabilityProbe();
        var encoderBackends = probe.SupportedBackends;
        var captureTargets = _senderSession.GetCaptureTargets();
        var target = captureTargets.FirstOrDefault(item => item.DeviceName == CaptureTarget) ?? captureTargets.FirstOrDefault();
        if (target is null)
        {
            HostStatus = "No capture target";
            HostDetail = "No display found.";
            return;
        }

        _hostAgent.ApplyConfiguration(new ControlPlaneAgentConfiguration(
            Enabled: true,
            BaseUrl: ControlPlaneUrl,
            DisplayName: Environment.MachineName,
            Region: "global",
            DirectPort: 5001,
            SenderBusy: _senderSession.GetSnapshot().Sending,
            EncoderPath: _senderSession.GetSnapshot().EncoderPath,
            Codec: _senderSession.GetSnapshot().Codec,
            Resolution: _senderSession.GetSnapshot().Resolution,
            CaptureFps: _senderSession.GetSnapshot().CaptureFps,
            EncodeFps: _senderSession.GetSnapshot().EncodeFps,
            ReceiverDecodeFps: _senderSession.GetSnapshot().ReceiverDecodeFps,
            PulseEstimateMs: _senderSession.GetSnapshot().PulseToAndroidEstimateMs,
            InputEstimateMs: _senderSession.GetSnapshot().InputToAndroidEstimateMs,
            FramesDropped: _senderSession.GetSnapshot().FramesDropped,
            PacketsSent: _senderSession.GetSnapshot().PacketsSent,
            SupportsHevc: probe.SupportsAdvertisedEncodeCodec(WindowsVideoCodec.H265Hevc),
            SupportsAudio: true,
            SupportsGamepad: true,
            EncoderBackends: encoderBackends,
            Capabilities: new ControlPlaneHostCapabilities(
                CpuModel: null,
                GpuModel: null,
                RamGb: 0,
                MaxWidth: target.Bounds.Width,
                MaxHeight: target.Bounds.Height,
                MaxFps: 60,
                SupportedEncodeCodecs: probe.SupportedEncodeCodecs,
                SupportedDecodeCodecs: new[] { WindowsVideoCodec.H265Hevc.ToMimeType(), WindowsVideoCodec.H264Avc.ToMimeType() },
                SupportedEncoderBackends: encoderBackends,
                LanAddresses: Array.Empty<string>())));

        HostRunning = true;
        SelectedTabIndex = 0;
        HostStatus = "Host agent enabled";
        HostDetail = $"Capture {target.UiLabel}; waiting for lease.";
        HostCode = GetHostCode(_hostAgent.GetSnapshot().HostId);
        HostSnapshot = "Agent enabled";
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(DiagnosticsText)));
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(PlaybackTitle)));
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(PlaybackDescription)));
        await Task.CompletedTask;
    }

    public Task StopHostAsync()
    {
        _senderSession.Stop();
        _hostAgent.ApplyConfiguration(ControlPlaneAgentConfiguration.Disabled);
        HostRunning = false;
        HostStatus = "Stopped";
        HostDetail = "Host agent disabled";
        HostCode = "-";
        HostSnapshot = "-";
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(DiagnosticsText)));
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(PlaybackTitle)));
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(PlaybackDescription)));
        return Task.CompletedTask;
    }

    public async Task RefreshHostsAsync()
    {
        try
        {
            var hosts = await _controlPlaneClient.ListHostsAsync(ControlPlaneUrl);
            Hosts.Clear();
            foreach (var host in hosts)
            {
                Hosts.Add(host);
            }

            TrySelectHostByCode();

            ClientStatus = $"Loaded {Hosts.Count} hosts";
            ClientDetail = "Host list refreshed.";
            RefreshClientTelemetry();
        }
        catch (Exception ex)
        {
            ClientStatus = "Host list failed";
            ClientDetail = ex.Message;
            RefreshClientTelemetry();
        }
    }

    public async Task LoginDemoAsync(string email, string password)
    {
        try
        {
            await _controlPlaneClient.LoginUserAsync(ControlPlaneUrl, email, password);
            ClientStatus = $"Auth as {email}";
            ClientDetail = "Demo auth ok.";
            RefreshClientTelemetry();
        }
        catch (Exception ex)
        {
            ClientStatus = "Auth failed";
            ClientDetail = ex.Message;
            RefreshClientTelemetry();
        }
    }

    public async Task RestoreManagedSessionAsync()
    {
        _clientAutoRestoreAttempted = true;
        var session = _controlPlaneClient.GetManagedSessionState(ControlPlaneUrl);
        if (session is null)
        {
            RefreshClientTelemetry();
            return;
        }

        try
        {
            var instructions = await _controlPlaneClient.ResumeManagedSessionAsync(ControlPlaneUrl, session.SessionId, session.SessionToken);
            SelectedTabIndex = 1;
            if (OperatingSystem.IsWindows() &&
                _clientRuntime.EnsureReceiverEndpoint(DesktopClientListenPort, out _, out var receiverPort))
            {
                var relayRegistrationRoute = TryBuildRelayRoute(session.SessionId, session.SessionToken, instructions.RelayHost, instructions.RelayPort);
                var relayRoute = BuildManagedRelayRoute(instructions.RouteKind, session.SessionId, session.SessionToken, instructions.RelayHost, instructions.RelayPort);
                _clientRuntime.Start(receiverPort, relayRegistrationRoute, relayRoute);
            }

            ApplyClientSession(
                status: "Session restored",
                detail: $"Restored {instructions.HostDisplayName}.",
                sessionId: instructions.SessionId,
                routeKind: instructions.RouteKind,
                endpoint: $"{instructions.StreamHost}:{instructions.StreamPort}",
                codec: session.CodecPreference ?? WindowsVideoCodec.H265Hevc.ToMimeType(),
                routeState: instructions.RouteState,
                health: instructions.SessionHealth,
                healthReason: instructions.SessionHealthReason,
                routeHint: instructions.RouteActionHint,
                routeReason: instructions.RouteActionReason,
                natStatus: instructions.NatStatus,
                hostNatAgeSeconds: instructions.HostNatProbeAgeSeconds,
                clientNatAgeSeconds: instructions.ClientNatProbeAgeSeconds,
                natProbeFresh: instructions.NatProbeFresh,
                receiverTelemetryAgeSeconds: instructions.ReceiverTelemetryAgeSeconds,
                senderTelemetryAgeSeconds: instructions.SenderTelemetryAgeSeconds,
                relayEndpoint: instructions.RelayHost is not null && instructions.RelayPort is not null ? $"{instructions.RelayHost}:{instructions.RelayPort}" : "-",
                receiverEndpoint: instructions.ReceiverHost is not null && instructions.ReceiverPort is not null ? $"{instructions.ReceiverHost}:{instructions.ReceiverPort}" : "-",
                recommendedSyncDelaySeconds: instructions.RecommendedSyncDelaySeconds,
                routeVersion: instructions.RouteVersion);
        }
        catch (Exception ex)
        {
            ClientStatus = "Restore failed";
            ClientDetail = ex.Message;
            RefreshClientTelemetry();
        }
    }

    public async Task ConnectByCodeAsync()
    {
        var localReceiverStarted = false;
        try
        {
            if (string.IsNullOrWhiteSpace(SelectedHostCode))
            {
                ClientStatus = "Code required";
                ClientDetail = "Enter host code.";
                RefreshClientTelemetry();
                return;
            }

            if (Hosts.Count == 0)
            {
                await RefreshHostsAsync();
            }

            var host = Hosts.FirstOrDefault(item => string.Equals(item.HostCode, SelectedHostCode.Trim(), StringComparison.OrdinalIgnoreCase));
            if (host is null)
            {
                ClientStatus = "Host not found";
                ClientDetail = $"No host with code {SelectedHostCode.Trim().ToUpperInvariant()}.";
                RefreshClientTelemetry();
                return;
            }

            SelectedHost = host;
            var desiredStream = BuildDesiredStream();
            var codec = TryParseCodec(SelectedCodec)?.ToMimeType() ?? WindowsVideoCodec.H265Hevc.ToMimeType();
            if (!OperatingSystem.IsWindows())
            {
                ClientStatus = "Windows-only backend";
                ClientDetail = "Desktop client playback requires Windows APIs.";
                RefreshClientTelemetry();
                return;
            }

            if (!_clientRuntime.EnsureReceiverEndpoint(DesktopClientListenPort, out var receiverHost, out var receiverPort))
            {
                ClientStatus = "Receiver endpoint unavailable";
                ClientDetail = "Could not resolve a LAN IPv4 address for desktop playback.";
                RefreshClientTelemetry();
                return;
            }

            _clientRuntime.Start(receiverPort, relayRegistrationRoute: null, relayRoute: null);
            localReceiverStarted = true;
            var lease = await _controlPlaneClient.CreateSessionAsync(
                baseUrl: ControlPlaneUrl,
                hostId: host.HostId,
                clientLabel: $"{Environment.MachineName} desktop",
                clientRegion: "global",
                codecPreference: codec,
                preferRelay: false,
                audioRequested: true,
                controllerCount: 0,
                leaseMinutes: 30,
                receiverAddress: receiverHost,
                receiverPort: receiverPort,
                desiredStream: desiredStream,
                clientCapabilities: new CPC.DesktopControlPlaneClientCapabilities(
                    SupportedDecodeCodecs: new[] { WindowsVideoCodec.H265Hevc.ToMimeType(), WindowsVideoCodec.H264Avc.ToMimeType(), WindowsVideoCodec.Av1.ToMimeType() },
                    LanAddresses: new[] { receiverHost }));

            _controlPlaneClient.SaveManagedSessionState(new CPC.DesktopControlPlaneManagedSessionState(
                BaseUrl: ControlPlaneUrl,
                SessionId: lease.SessionId,
                SessionToken: lease.SessionToken,
                HostId: lease.HostId,
                HostDisplayName: lease.HostDisplayName,
                RouteKind: lease.RouteKind,
                RouteState: lease.RouteState,
                SessionHealth: lease.SessionHealth,
                SessionHealthReason: lease.SessionHealthReason,
                RouteActionHint: lease.RouteActionHint,
                RouteActionReason: lease.RouteActionReason,
                RouteFallbackReadyDurationSeconds: lease.RouteFallbackReadyDurationSeconds,
                RouteRecoveryReadyDurationSeconds: lease.RouteRecoveryReadyDurationSeconds,
                RecommendedSyncDelaySeconds: lease.RecommendedSyncDelaySeconds,
                TransportLossLevel: lease.TransportLossLevel,
                TransportAnomalyKind: lease.TransportAnomalyKind,
                TransportAnomalyReason: lease.TransportAnomalyReason,
                TransportAnomalyConfidence: lease.TransportAnomalyConfidence,
                ReceiverTelemetryAgeSeconds: lease.ReceiverTelemetryAgeSeconds,
                SenderTelemetryAgeSeconds: lease.SenderTelemetryAgeSeconds,
                RouteRecoveryCount: lease.RouteRecoveryCount,
                RouteRecoveryCooldownSeconds: lease.RouteRecoveryCooldownSeconds,
                NatStatus: lease.NatStatus,
                HostNatProbeAgeSeconds: lease.HostNatProbeAgeSeconds,
                ClientNatProbeAgeSeconds: lease.ClientNatProbeAgeSeconds,
                NatProbeFresh: lease.NatProbeFresh,
                RelayAddress: lease.RelayAddress,
                RelayPort: lease.RelayPort,
                ReceiverAddress: lease.ReceiverAddress,
                ReceiverPort: lease.ReceiverPort,
                ProbeAddress: lease.ProbeAddress,
                ProbePort: lease.ProbePort,
                ProbeToken: lease.ProbeToken,
                CodecPreference: codec,
                RouteVersion: lease.RouteVersion,
                RouteFallbackCount: lease.RouteFallbackCount,
                RouteFallbackCooldownSeconds: lease.RouteFallbackCooldownSeconds,
                LastRouteActionKind: lease.LastRouteActionKind,
                LastRouteActionReason: lease.LastRouteActionReason,
                LastRouteActionActor: lease.LastRouteActionActor,
                LastRouteActionUtc: lease.LastRouteActionUtc));

            var instructions = await _controlPlaneClient.GetConnectInstructionsAsync(ControlPlaneUrl, lease.SessionId, lease.SessionToken);
            SelectedTabIndex = 1;
            var relayRegistrationRoute = TryBuildRelayRoute(lease.SessionId, lease.SessionToken, instructions.RelayHost, instructions.RelayPort);
            var relayRoute = BuildManagedRelayRoute(instructions.RouteKind, lease.SessionId, lease.SessionToken, instructions.RelayHost, instructions.RelayPort);
            _clientRuntime.Start(receiverPort, relayRegistrationRoute, relayRoute);
            ApplyClientSession(
                status: "Session prepared",
                detail: $"Host {host.DisplayName} selected. Route {instructions.RouteKind}.",
                sessionId: lease.SessionId,
                routeKind: instructions.RouteKind,
                endpoint: $"{instructions.StreamHost}:{instructions.StreamPort}",
                codec: codec,
                routeState: instructions.RouteState,
                health: instructions.SessionHealth,
                healthReason: instructions.SessionHealthReason,
                routeHint: instructions.RouteActionHint,
                routeReason: instructions.RouteActionReason,
                natStatus: instructions.NatStatus,
                hostNatAgeSeconds: instructions.HostNatProbeAgeSeconds,
                clientNatAgeSeconds: instructions.ClientNatProbeAgeSeconds,
                natProbeFresh: instructions.NatProbeFresh,
                receiverTelemetryAgeSeconds: instructions.ReceiverTelemetryAgeSeconds,
                senderTelemetryAgeSeconds: instructions.SenderTelemetryAgeSeconds,
                relayEndpoint: instructions.RelayHost is not null && instructions.RelayPort is not null ? $"{instructions.RelayHost}:{instructions.RelayPort}" : "-",
                receiverEndpoint: instructions.ReceiverHost is not null && instructions.ReceiverPort is not null ? $"{instructions.ReceiverHost}:{instructions.ReceiverPort}" : "-",
                recommendedSyncDelaySeconds: instructions.RecommendedSyncDelaySeconds,
                routeVersion: instructions.RouteVersion);
        }
        catch (Exception ex)
        {
            if (localReceiverStarted)
            {
                _clientRuntime.Stop();
            }
            ClientStatus = "Connect failed";
            ClientDetail = ex.Message;
            RefreshClientTelemetry();
        }
    }

    public Task StopClientAsync()
    {
        var session = _controlPlaneClient.GetManagedSessionState(ControlPlaneUrl);
        if (session is null)
        {
            ClientStatus = "No session";
            ClientDetail = "Nothing to stop.";
            RefreshClientTelemetry();
            return Task.CompletedTask;
        }

        return StopManagedSessionAsync(session);
    }

    public void OpenClientPlaybackWindow()
    {
        _clientRuntime.ShowPlaybackWindow();
        RefreshClientTelemetry();
    }

    public void HideClientPlaybackWindow()
    {
        _clientRuntime.HidePlaybackWindow();
        RefreshClientTelemetry();
    }

    public void ToggleDiagnostics()
    {
        DiagnosticsVisible = !DiagnosticsVisible;
        _uiPreferences.DiagnosticsVisible = DiagnosticsVisible;
        _uiPreferences.Save();
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(DiagnosticsText)));
    }

    public void ToggleAdvanced()
    {
        AdvancedVisible = !AdvancedVisible;
        _uiPreferences.AdvancedVisible = AdvancedVisible;
        _uiPreferences.Save();
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(DiagnosticsText)));
    }

    public void OnHostSnapshotChanged()
    {
        RefreshHostTelemetry(_hostAgent.GetSnapshot());
    }

    private void RefreshHostTelemetry()
    {
        RefreshHostTelemetry(_hostAgent.GetSnapshot());
    }

    private void RefreshHostTelemetry(ControlPlaneAgentSnapshot snapshot)
    {
        var senderSnapshot = _senderSession.GetSnapshot();
        HostStatus = snapshot.Status;
        HostCode = GetHostCode(snapshot.HostId);
        HostDetail = string.Join(" · ", new[]
        {
            snapshot.LeaseStatus,
            snapshot.LeaseRouteKind,
            snapshot.LeaseReceiverEndpoint,
            snapshot.LeaseNatStatus,
        }.Where(static item => !string.IsNullOrWhiteSpace(item) && item != "-"));
        HostSnapshot = string.Join(Environment.NewLine, new[]
        {
            $"Host code: {HostCode}",
            $"Status: {senderSnapshot.Status}",
            $"Profile: {GetSelectedHostPresetSpec().ProtocolPreset}",
            $"Auto encoder: {senderSnapshot.AutoEncoderSelected}",
            $"Encoder path: {senderSnapshot.EncoderPath}",
            $"Selected codec: {senderSnapshot.Codec}",
            $"Selected route: {snapshot.LeaseRouteKind}",
            $"Sender target: {FormatMetric(senderSnapshot.TargetFps, " fps")}",
            $"Sender capture: {FormatMetric(senderSnapshot.CaptureFps, " fps")}",
            $"Sender submit: {FormatMetric(senderSnapshot.SubmitFps, " fps")}",
            $"Sender encode: {FormatMetric(senderSnapshot.EncodeFps, " fps")}",
            $"Native stages: {senderSnapshot.NativeStageStats}",
            $"Native cadence: dxgi timeouts {senderSnapshot.NativeDxgiTimeouts} | paced skips {senderSnapshot.NativePacedSkips}",
            $"Pulse -> Android: {FormatLatencyMetric(senderSnapshot.PulseToAndroidEstimateMs, senderSnapshot.ReceiverFeedbackAgeMs)}",
            $"Input -> Android: {FormatLatencyMetric(senderSnapshot.InputToAndroidEstimateMs, senderSnapshot.ReceiverFeedbackAgeMs)}",
            $"Receiver decode: {FormatMetric(senderSnapshot.ReceiverDecodeFps, " fps")}",
            $"Feedback age: {FormatMetric(senderSnapshot.ReceiverFeedbackAgeMs, " ms")}",
            $"Gamepad status: {senderSnapshot.GamepadStatus}",
            $"Gamepad input: {senderSnapshot.GamepadInput}",
            $"Latency mode: {(snapshot.LeaseStatus == "Active" ? "Managed" : "Idle")}",
            $"Adaptive downscale: {(HostAdaptiveStreamingEnabled ? "On" : "Off")}",
            $"Lease session: {snapshot.LeaseSessionId}",
            $"Lease receiver: {snapshot.LeaseReceiverEndpoint}",
        });
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(DiagnosticsText)));
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(PlaybackTitle)));
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(PlaybackDescription)));
    }

    private void HandleHostSnapshotChanged(ControlPlaneAgentSnapshot snapshot)
    {
        OnHostSnapshotChanged();
        if (snapshot.LeaseReceiverRegistered && snapshot.LeaseHostReady && snapshot.LeaseStatus == "Active")
        {
            TryStartSender(snapshot);
        }
        else if (snapshot.LeaseStatus is "No lease" or "Stopped" or "Expired")
        {
            _senderSession.Stop();
        }
    }

    private static string GetHostCode(string hostId)
    {
        const string prefix = "host_";
        if (string.IsNullOrWhiteSpace(hostId))
        {
            return "-";
        }

        var trimmed = hostId.Trim();
        if (trimmed.StartsWith(prefix, StringComparison.OrdinalIgnoreCase))
        {
            var body = trimmed[prefix.Length..];
            return body.Length <= 4 ? body : body[..4];
        }

        return trimmed.Length <= 4 ? trimmed : trimmed[..4];
    }

    private static string FormatMetric(int value, string suffix = " ms") =>
        value < 0 ? "-" : $"{value}{suffix}";

    private static string FormatLatencyMetric(int value, int ageMs) =>
        value < 0 ? "-" : ageMs >= 0 ? $"{value} ms ({ageMs} ms ago)" : $"{value} ms";

    private static string FormatSeconds(int value) =>
        value < 0 ? "-" : $"{value} s";

    private static bool ShouldUseRelayRoute(string? routeKind) =>
        !string.IsNullOrWhiteSpace(routeKind) &&
        routeKind.Contains("relay", StringComparison.OrdinalIgnoreCase);

    private static RelayTransportRoute? BuildManagedRelayRoute(string? routeKind, string sessionId, string sessionToken, string? relayHost, int? relayPort) =>
        ShouldUseRelayRoute(routeKind)
            ? TryBuildRelayRoute(sessionId, sessionToken, relayHost, relayPort)
            : null;

    private static RelayTransportRoute? TryBuildRelayRoute(string sessionId, string sessionToken, string? relayHost, int? relayPort)
    {
        if (string.IsNullOrWhiteSpace(sessionId) ||
            string.IsNullOrWhiteSpace(sessionToken) ||
            string.IsNullOrWhiteSpace(relayHost) ||
            relayPort is null or <= 0 or > 65535)
        {
            return null;
        }

        return new RelayTransportRoute(sessionId.Trim(), sessionToken.Trim(), relayHost.Trim(), relayPort.Value);
    }

    private void TrySelectHostByCode()
    {
        var code = SelectedHostCode.Trim();
        if (string.IsNullOrWhiteSpace(code) || Hosts.Count == 0)
        {
            return;
        }

        var host = Hosts.FirstOrDefault(item => string.Equals(item.HostCode, code, StringComparison.OrdinalIgnoreCase));
        if (host is not null)
        {
            _selectedHost = host;
            PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(SelectedHost)));
        }
    }

    private void TryStartSender(ControlPlaneAgentSnapshot snapshot)
    {
        if (!OperatingSystem.IsWindows() || _senderSession.GetSnapshot().Sending)
        {
            return;
        }

        var receiverEndpoint = ParseEndpoint(snapshot.LeaseReceiverEndpoint);
        if (receiverEndpoint is null)
        {
            HostDetail = "Lease receiver endpoint missing.";
            return;
        }

        RelayTransportRoute? relayRoute = null;
        if (snapshot.LeaseRouteKind.Contains("relay", StringComparison.OrdinalIgnoreCase))
        {
            var relayEndpoint = ParseEndpoint(snapshot.LeaseRelayEndpoint);
            if (relayEndpoint is not null)
            {
                relayRoute = new RelayTransportRoute(snapshot.LeaseSessionId, snapshot.LeaseSessionToken, relayEndpoint.Address.ToString(), relayEndpoint.Port);
            }
        }

        var codec = TryParseCodec(snapshot.LeaseCodecPreference) ?? TryParseCodec(SelectedCodec) ?? WindowsVideoCodec.H265Hevc;
        var preset = GetSelectedHostPresetSpec();
        var encoderBackend = TryParseEncoderBackend(SelectedEncoder);
        var captureTarget = _senderSession.GetCaptureTargets().FirstOrDefault(item => item.DeviceName == CaptureTarget)
            ?? _senderSession.GetCaptureTargets().FirstOrDefault();
        if (captureTarget is null)
        {
            HostDetail = "No capture target found.";
            return;
        }

        try
        {
            _senderSession.Start(
                host: receiverEndpoint.Address.ToString(),
                port: receiverEndpoint.Port,
                captureTargetDeviceName: captureTarget.DeviceName,
                encoderBackend: encoderBackend,
                codec: codec,
                spec: preset,
                audioEnabled: true,
                captureCursorInStream: false,
                latencyPulseFlashEnabled: false,
                adaptiveEnabled: HostAdaptiveStreamingEnabled,
                relayRoute: relayRoute);
        }
        catch (Exception ex)
        {
            HostStatus = "Sender failed";
            HostDetail = ex.Message;
        }
    }

    private async Task StopManagedSessionAsync(CPC.DesktopControlPlaneManagedSessionState session)
    {
        try
        {
            await _controlPlaneClient.StopSessionAsync(ControlPlaneUrl, session.SessionId, session.SessionToken, "avalonia_client_stop");
            _controlPlaneClient.ClearManagedSessionState(ControlPlaneUrl);
            _clientRuntime.Stop();
            ClientStatus = "Session stopped";
            ClientDetail = $"Stopped {session.HostDisplayName}.";
            ClientSession = "-";
            ClientRoute = "-";
            ClientEndpoint = "-";
            ClientCodec = "-";
            ClientSnapshot = "-";
            RefreshClientTelemetry();
        }
        catch (Exception ex)
        {
            ClientStatus = "Stop failed";
            ClientDetail = ex.Message;
            RefreshClientTelemetry();
        }
    }

    private void RefreshClientRuntimeTelemetry()
    {
        var runtimeSnapshot = _clientRuntime.GetSnapshot();
        if (runtimeSnapshot.Listening && !string.Equals(ClientSession, "-", StringComparison.Ordinal))
        {
            ClientSnapshot = string.Join(Environment.NewLine, new[]
            {
                $"Session: {ClientSession}",
                $"Route: {ClientRoute}",
                $"Endpoint: {ClientEndpoint}",
                $"Codec: {ClientCodec}",
                $"Playback status: {runtimeSnapshot.PlaybackStatus}",
                $"Receiver status: {runtimeSnapshot.Status}",
                $"Receiver backend: {runtimeSnapshot.PlaybackBackend}",
                $"Receiver decode: {runtimeSnapshot.DecodeMode}",
                $"Resolution: {runtimeSnapshot.Resolution}",
                $"Target FPS: {(runtimeSnapshot.TargetFps > 0 ? runtimeSnapshot.TargetFps : 0)}",
                $"Bitrate: {(runtimeSnapshot.BitrateMbps > 0 ? $"{runtimeSnapshot.BitrateMbps:0.0} Mbps" : "-")}",
                $"Remote endpoint: {runtimeSnapshot.RemoteEndpoint}",
                $"Packets: {runtimeSnapshot.PacketsReceived}",
                $"Frames: {runtimeSnapshot.FramesAssembled}",
                $"Drops: {runtimeSnapshot.TotalDroppedFrames}",
                $"Queued AU: {runtimeSnapshot.StreamQueuedAccessUnits}",
                $"Waiting keyframe: {runtimeSnapshot.WaitingForKeyFrame}",
                $"Arrival / decode / present: {FormatMetric(runtimeSnapshot.ArrivalDeltaMs)} / {FormatMetric(runtimeSnapshot.DecodeDeltaMs)} / {FormatMetric(runtimeSnapshot.PresentDeltaMs)}",
                $"Pulse -> PC: {FormatMetric(runtimeSnapshot.PulseToPcEstimateMs)}",
                $"Input -> PC: {FormatMetric(runtimeSnapshot.TapToPcEstimateMs)}",
                $"Last packet: {runtimeSnapshot.LastPacketType}",
                $"Playback error: {runtimeSnapshot.LastPlaybackError}",
            });
            _clientRuntime.RefreshPlaybackWindow();
            PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(ClientSnapshot)));
            PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(DiagnosticsText)));
            PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(PlaybackDescription)));
        }

        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(CanOpenClientPlaybackWindow)));
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(IsClientPlaybackWindowVisible)));
    }

    private void ApplyClientSession(
        string status,
        string detail,
        string sessionId,
        string routeKind,
        string endpoint,
        string codec,
        string routeState,
        string health,
        string healthReason,
        string routeHint,
        string routeReason,
        string natStatus,
        int hostNatAgeSeconds,
        int clientNatAgeSeconds,
        bool natProbeFresh,
        int receiverTelemetryAgeSeconds,
        int senderTelemetryAgeSeconds,
        string relayEndpoint,
        string receiverEndpoint,
        int recommendedSyncDelaySeconds,
        int routeVersion)
    {
        ClientStatus = status;
        ClientDetail = detail;
        ClientSession = sessionId;
        ClientRoute = routeKind;
        ClientEndpoint = endpoint;
        ClientCodec = codec;
        ClientSnapshot = string.Join(Environment.NewLine, new[]
        {
            $"Session: {sessionId}",
            $"Route: {routeKind} / {routeState}",
            $"Stream endpoint: {endpoint}",
            $"Receiver endpoint: {receiverEndpoint}",
            $"Relay endpoint: {relayEndpoint}",
            $"Codec: {codec}",
            $"Health: {health}",
            $"Health reason: {healthReason}",
            $"Route hint: {routeHint}",
            $"Route reason: {routeReason}",
            $"NAT: {natStatus} ({(natProbeFresh ? "fresh" : "stale")})",
            $"Host probe age: {FormatSeconds(hostNatAgeSeconds)}",
            $"Client probe age: {FormatSeconds(clientNatAgeSeconds)}",
            $"Receiver telemetry age: {FormatSeconds(receiverTelemetryAgeSeconds)}",
            $"Sender telemetry age: {FormatSeconds(senderTelemetryAgeSeconds)}",
            $"Recommended sync delay: {FormatSeconds(recommendedSyncDelaySeconds)}",
            $"Route version: {routeVersion}",
        });
        RefreshClientTelemetry();
    }

    private void RefreshClientTelemetry()
    {
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(ClientPresetSummary)));
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(SelectedHostSummary)));
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(CanRestoreClientSession)));
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(CanOpenClientPlaybackWindow)));
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(IsClientPlaybackWindowVisible)));
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(DiagnosticsText)));
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(PlaybackTitle)));
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(PlaybackDescription)));
    }

    private CPC.DesktopControlPlaneDesiredStreamRequest BuildDesiredStream()
    {
        var preset = SelectedClientPreset switch
        {
            "Low Latency" => new CPC.DesktopControlPlaneDesiredStreamRequest(1280, 720, 60, 8_500_000, CaptureCursor: false, AdaptiveMode: true, PreferredCodecs: new[] { WindowsVideoCodec.H264Avc.ToMimeType(), WindowsVideoCodec.H265Hevc.ToMimeType() }, PresetId: "low_latency"),
            "Quality" => new CPC.DesktopControlPlaneDesiredStreamRequest(1920, 1080, 60, 16_500_000, CaptureCursor: false, AdaptiveMode: false, PreferredCodecs: new[] { WindowsVideoCodec.Av1.ToMimeType(), WindowsVideoCodec.H265Hevc.ToMimeType(), WindowsVideoCodec.H264Avc.ToMimeType() }, PresetId: "quality"),
            _ => new CPC.DesktopControlPlaneDesiredStreamRequest(1600, 900, 60, 12_000_000, CaptureCursor: false, AdaptiveMode: true, PreferredCodecs: new[] { WindowsVideoCodec.H265Hevc.ToMimeType(), WindowsVideoCodec.H264Avc.ToMimeType() }, PresetId: "balanced"),
        };

        return preset;
    }

    private static IPEndPoint? ParseEndpoint(string value)
    {
        if (string.IsNullOrWhiteSpace(value) || value == "-")
        {
            return null;
        }

        var endpointText = value.Split(' ', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries).FirstOrDefault();
        if (string.IsNullOrWhiteSpace(endpointText))
        {
            return null;
        }

        var parts = endpointText.Split(':', 2);
        if (parts.Length != 2 || !IPAddress.TryParse(parts[0], out var address) || !int.TryParse(parts[1], out var port))
        {
            return null;
        }

        return new IPEndPoint(address, port);
    }

    private static WindowsVideoCodec? TryParseCodec(string? label)
        => WindowsVideoCodecExtensions.TryParse(label);

    private static WindowsSenderPreset? TryParsePreset(string? label)
    {
        if (string.IsNullOrWhiteSpace(label))
        {
            return null;
        }

        return Enum.TryParse<WindowsSenderPreset>(label.Trim(), ignoreCase: true, out var preset)
            ? preset
            : null;
    }

    private static WindowsSenderEncoderBackend TryParseEncoderBackend(string? label)
    {
        if (string.IsNullOrWhiteSpace(label))
        {
            return WindowsSenderEncoderBackend.Auto;
        }

        foreach (var backend in Enum.GetValues<WindowsSenderEncoderBackend>())
        {
            if (string.Equals(label.Trim(), backend.ToUiLabel(), StringComparison.OrdinalIgnoreCase) ||
                string.Equals(label.Trim(), backend.ToString(), StringComparison.OrdinalIgnoreCase))
            {
                return backend;
            }
        }

        return WindowsSenderEncoderBackend.Auto;
    }

    private WindowsSenderPresetSpec GetSelectedHostPresetSpec()
    {
        var baseSpec = TryParsePreset(SelectedPreset)?.ToSpec() ?? WindowsSenderPreset.Game.ToSpec();
        return GetHostProfileOverride(baseSpec.ProtocolPreset) is { } profile
            ? baseSpec with
            {
                TargetWidth = profile.Width,
                TargetHeight = profile.Height,
                TargetFps = profile.Fps,
                TargetBitrateBps = profile.BitrateBps,
            }
            : baseSpec;
    }

    private HostProfileOverride? GetHostProfileOverride(string protocolPreset)
    {
        if (!TryParsePositiveInt(_hostProfileWidth, out var width) ||
            !TryParsePositiveInt(_hostProfileHeight, out var height) ||
            !TryParsePositiveInt(_hostProfileFps, out var fps) ||
            !TryParseBitrateMbps(_hostProfileBitrateMbps, out var bitrateBps))
        {
            return null;
        }

        width = Math.Max(64, width - (width % 2));
        height = Math.Max(64, height - (height % 2));
        fps = Math.Clamp(fps, 1, 240);
        bitrateBps = Math.Max(400_000, bitrateBps);
        return new HostProfileOverride(width, height, fps, bitrateBps);
    }

    private void LoadHostProfileFields()
    {
        var selected = TryParsePreset(SelectedPreset) ?? WindowsSenderPreset.Game;
        var baseSpec = selected.ToSpec();
        int width;
        int height;
        int fps;
        int bitrateBps;

        if (selected == WindowsSenderPreset.Media)
        {
            width = _uiPreferences.HostMediaWidth ?? baseSpec.TargetWidth;
            height = _uiPreferences.HostMediaHeight ?? baseSpec.TargetHeight;
            fps = _uiPreferences.HostMediaFps ?? baseSpec.TargetFps;
            bitrateBps = _uiPreferences.HostMediaBitrateBps ?? baseSpec.TargetBitrateBps;
        }
        else
        {
            width = _uiPreferences.HostGameWidth ?? baseSpec.TargetWidth;
            height = _uiPreferences.HostGameHeight ?? baseSpec.TargetHeight;
            fps = _uiPreferences.HostGameFps ?? baseSpec.TargetFps;
            bitrateBps = _uiPreferences.HostGameBitrateBps ?? baseSpec.TargetBitrateBps;
        }

        SetProperty(ref _hostProfileWidth, width.ToString(), nameof(HostProfileWidth));
        SetProperty(ref _hostProfileHeight, height.ToString(), nameof(HostProfileHeight));
        SetProperty(ref _hostProfileFps, fps.ToString(), nameof(HostProfileFps));
        SetProperty(ref _hostProfileBitrateMbps, (bitrateBps / 1_000_000.0).ToString("0.0"), nameof(HostProfileBitrateMbps));
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(SelectedPresetSummary)));
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(DiagnosticsText)));
    }

    private void SaveHostProfileFields()
    {
        var selected = TryParsePreset(SelectedPreset) ?? WindowsSenderPreset.Game;
        _ = TryParsePositiveInt(_hostProfileWidth, out var width);
        _ = TryParsePositiveInt(_hostProfileHeight, out var height);
        _ = TryParsePositiveInt(_hostProfileFps, out var fps);
        _ = TryParseBitrateMbps(_hostProfileBitrateMbps, out var bitrateBps);

        if (selected == WindowsSenderPreset.Media)
        {
            _uiPreferences.HostMediaWidth = width > 0 ? width : null;
            _uiPreferences.HostMediaHeight = height > 0 ? height : null;
            _uiPreferences.HostMediaFps = fps > 0 ? fps : null;
            _uiPreferences.HostMediaBitrateBps = bitrateBps > 0 ? bitrateBps : null;
        }
        else
        {
            _uiPreferences.HostGameWidth = width > 0 ? width : null;
            _uiPreferences.HostGameHeight = height > 0 ? height : null;
            _uiPreferences.HostGameFps = fps > 0 ? fps : null;
            _uiPreferences.HostGameBitrateBps = bitrateBps > 0 ? bitrateBps : null;
        }

        _uiPreferences.Save();
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(SelectedPresetSummary)));
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(DiagnosticsText)));
    }

    private static bool TryParsePositiveInt(string? value, out int result)
    {
        result = 0;
        return !string.IsNullOrWhiteSpace(value) && int.TryParse(value, out result) && result > 0;
    }

    private static bool TryParseBitrateMbps(string? value, out int bitrateBps)
    {
        bitrateBps = 0;
        if (string.IsNullOrWhiteSpace(value))
        {
            return false;
        }

        var normalized = value.Trim().Replace(',', '.');
        if (!double.TryParse(normalized, out var mbps) || mbps <= 0)
        {
            return false;
        }

        bitrateBps = (int)Math.Round(mbps * 1_000_000.0);
        return bitrateBps > 0;
    }

    private bool SetProperty<T>(ref T storage, T value, [CallerMemberName] string? propertyName = null)
    {
        if (EqualityComparer<T>.Default.Equals(storage, value))
        {
            return false;
        }

        storage = value;
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(propertyName));
        return true;
    }

    public void Dispose()
    {
        if (_disposed)
        {
            return;
        }

        _disposed = true;
        _telemetryTimer.Stop();
        _clientRuntime.Dispose();
        _hostAgent.Dispose();
        _controlPlaneClient.Dispose();
        _senderSession.Dispose();
    }
}
