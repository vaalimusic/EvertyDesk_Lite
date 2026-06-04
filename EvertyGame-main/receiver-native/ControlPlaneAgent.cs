namespace ReceiverNative;

using System.Net;
using System.Net.Http;
using System.Net.Http.Json;
using System.Net.NetworkInformation;
using System.Net.Sockets;
using System.Text.Json;

internal sealed record ControlPlaneAgentConfiguration(
    bool Enabled,
    string BaseUrl,
    string DisplayName,
    string Region,
    int DirectPort,
    bool SenderBusy,
    string EncoderPath,
    string Codec,
    string Resolution,
    int CaptureFps,
    int EncodeFps,
    int ReceiverDecodeFps,
    int PulseEstimateMs,
    int InputEstimateMs,
    long FramesDropped,
    long PacketsSent,
    bool SupportsHevc,
    bool SupportsAudio,
    bool SupportsGamepad,
    string[] EncoderBackends,
    ControlPlaneHostCapabilities Capabilities)
{
    public static ControlPlaneAgentConfiguration Disabled { get; } = new(
        Enabled: false,
        BaseUrl: string.Empty,
        DisplayName: Environment.MachineName,
        Region: "global",
        DirectPort: 5001,
        SenderBusy: false,
        EncoderPath: "-",
        Codec: "-",
        Resolution: "-",
        CaptureFps: 0,
        EncodeFps: 0,
        ReceiverDecodeFps: 0,
        PulseEstimateMs: -1,
        InputEstimateMs: -1,
        FramesDropped: 0,
        PacketsSent: 0,
        SupportsHevc: true,
        SupportsAudio: true,
        SupportsGamepad: true,
        EncoderBackends: Array.Empty<string>(),
        Capabilities: new ControlPlaneHostCapabilities());
}

internal sealed record ControlPlaneHostCapabilities(
    string? CpuModel = null,
    string? GpuModel = null,
    int RamGb = 0,
    int MaxWidth = 0,
    int MaxHeight = 0,
    int MaxFps = 0,
    string[]? SupportedEncodeCodecs = null,
    string[]? SupportedDecodeCodecs = null,
    string[]? SupportedEncoderBackends = null,
    string[]? LanAddresses = null);

internal sealed record ControlPlaneAgentSnapshot
{
    public bool Enabled { get; init; }
    public string BaseUrl { get; init; } = "-";
    public string Status { get; init; } = "Disabled";
    public string HostId { get; init; } = "-";
    public string LeaseStatus { get; init; } = "-";
    public string LeaseSessionId { get; init; } = "-";
    public string LeaseSessionToken { get; init; } = string.Empty;
    public string LeaseClientLabel { get; init; } = "-";
    public DateTimeOffset? LeaseExpiresUtc { get; init; }
    public string LeaseReceiverEndpoint { get; init; } = "-";
    public string LeaseRouteKind { get; init; } = "-";
    public string LeaseRelayEndpoint { get; init; } = "-";
    public string LeaseRelayRegion { get; init; } = "-";
    public string LeaseProbeEndpoint { get; init; } = "-";
    public string LeaseProbeToken { get; init; } = string.Empty;
    public string LeaseNatStatus { get; init; } = "-";
    public bool LeaseReceiverRegistered { get; init; }
    public bool LeaseHostReady { get; init; }
    public string LeaseCodecPreference { get; init; } = "-";
    public int LeaseRequestedWidth { get; init; }
    public int LeaseRequestedHeight { get; init; }
    public int LeaseRequestedFps { get; init; }
    public int LeaseRequestedBitrateBps { get; init; }
    public bool? LeaseCaptureCursor { get; init; }
    public bool? LeaseAdaptiveMode { get; init; }
    public bool LeaseUnattendedAuthorized { get; init; }
    public int HeartbeatIntervalSeconds { get; init; } = 5;
    public DateTimeOffset? LastHeartbeatUtc { get; init; }
    public string LastError { get; init; } = "-";
}

internal sealed class ControlPlaneAgent : IDisposable
{
    private readonly object _sync = new();
    private readonly HttpClient _httpClient = new() { Timeout = TimeSpan.FromSeconds(5) };
    private readonly string _statePath;
    private readonly JsonSerializerOptions _jsonOptions = new(JsonSerializerDefaults.Web);

    private ControlPlaneAgentConfiguration _configuration = ControlPlaneAgentConfiguration.Disabled;
    private ControlPlaneRegistrationState? _registrationState;
    private CancellationTokenSource? _loopCts;
    private Task? _loopTask;
    private ControlPlaneAgentSnapshot _snapshot = new();
    private DateTimeOffset _lastTelemetryUtc = DateTimeOffset.MinValue;
    private DateTimeOffset _lastNatProbeUtc = DateTimeOffset.MinValue;
    private HostLeaseResponse? _lastEffectiveLease;
    private DateTimeOffset _lastEffectiveLeaseUtc = DateTimeOffset.MinValue;
    private string? _lastLoggedLeaseSessionId;
    private string? _lastLeaseGapSessionId;
    private string? _lastLoggedLeasePollState;
    private string? _lastLoggedSnapshotLeaseState;

    public event Action<ControlPlaneAgentSnapshot>? SnapshotChanged;

    public ControlPlaneAgent()
    {
        var directory = Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
            "EvertyNativeReceiver");
        Directory.CreateDirectory(directory);
        _statePath = Path.Combine(directory, "control-plane-agent.json");
    }

    public ControlPlaneAgentSnapshot GetSnapshot()
    {
        lock (_sync)
        {
            return _snapshot;
        }
    }

    public void ApplyConfiguration(ControlPlaneAgentConfiguration configuration)
    {
        var normalized = configuration with
        {
            BaseUrl = NormalizeBaseUrl(configuration.BaseUrl),
            DisplayName = string.IsNullOrWhiteSpace(configuration.DisplayName) ? Environment.MachineName : configuration.DisplayName.Trim(),
            Region = string.IsNullOrWhiteSpace(configuration.Region) ? "global" : configuration.Region.Trim(),
            EncoderBackends = configuration.EncoderBackends
                .Where(static item => !string.IsNullOrWhiteSpace(item))
                .Select(static item => item.Trim())
                .Distinct(StringComparer.OrdinalIgnoreCase)
                .ToArray(),
        };

        CancellationTokenSource? oldCts = null;
        var shouldStartLoop = false;

        lock (_sync)
        {
            var previousBaseUrl = _configuration.BaseUrl;
            var wasEnabled = _configuration.Enabled && !string.IsNullOrWhiteSpace(previousBaseUrl);
            _configuration = normalized;
            var isEnabled = normalized.Enabled && !string.IsNullOrWhiteSpace(normalized.BaseUrl);

            if (!isEnabled)
            {
                oldCts = _loopCts;
                _loopCts = null;
                _loopTask = null;
                _registrationState = null;
                var snapshot = _snapshot with
                {
                    Enabled = false,
                    BaseUrl = "-",
                    Status = "Disabled",
                    HostId = "-",
                    LeaseStatus = "-",
                    LeaseSessionId = "-",
                    LeaseSessionToken = string.Empty,
                    LeaseClientLabel = "-",
                    LeaseExpiresUtc = null,
                    LeaseReceiverEndpoint = "-",
                    LeaseRouteKind = "-",
                    LeaseRelayEndpoint = "-",
                    LeaseRelayRegion = "-",
                    LeaseProbeEndpoint = "-",
                    LeaseProbeToken = string.Empty,
                    LeaseNatStatus = "-",
                    LeaseReceiverRegistered = false,
                    LeaseHostReady = false,
                    LeaseCodecPreference = "-",
                    LeaseRequestedWidth = 0,
                    LeaseRequestedHeight = 0,
                    LeaseRequestedFps = 0,
                    LeaseRequestedBitrateBps = 0,
                    LeaseCaptureCursor = null,
                    LeaseAdaptiveMode = null,
                    LeaseUnattendedAuthorized = false,
                    LastError = "-",
                };
                _snapshot = snapshot;
                NotifySnapshotChanged(snapshot);
            }
            else
            {
                var snapshot = _snapshot with
                {
                    Enabled = true,
                    BaseUrl = normalized.BaseUrl,
                };
                _snapshot = snapshot;

                if (!wasEnabled || !string.Equals(previousBaseUrl, normalized.BaseUrl, StringComparison.OrdinalIgnoreCase) || _loopCts is null)
                {
                    oldCts = _loopCts;
                    _loopCts = new CancellationTokenSource();
                    _loopTask = Task.Run(() => RunLoopAsync(_loopCts.Token));
                    _registrationState = LoadRegistrationState(normalized.BaseUrl);
                    snapshot = _snapshot with
                    {
                        Status = "Connecting...",
                        HostId = _registrationState?.HostId ?? "-",
                        LeaseStatus = "-",
                        LeaseSessionId = "-",
                        LeaseSessionToken = string.Empty,
                        LeaseClientLabel = "-",
                        LeaseExpiresUtc = null,
                        LeaseReceiverEndpoint = "-",
                        LeaseRouteKind = "-",
                        LeaseRelayEndpoint = "-",
                        LeaseRelayRegion = "-",
                        LeaseProbeEndpoint = "-",
                        LeaseProbeToken = string.Empty,
                        LeaseNatStatus = "-",
                        LeaseReceiverRegistered = false,
                        LeaseHostReady = false,
                        LeaseCodecPreference = "-",
                        LeaseRequestedWidth = 0,
                        LeaseRequestedHeight = 0,
                        LeaseRequestedFps = 0,
                        LeaseRequestedBitrateBps = 0,
                        LeaseCaptureCursor = null,
                        LeaseAdaptiveMode = null,
                        LeaseUnattendedAuthorized = false,
                        LastError = "-",
                    };
                    _snapshot = snapshot;
                    shouldStartLoop = true;
                    NotifySnapshotChanged(snapshot);
                }
            }
        }

        if (oldCts is not null)
        {
            oldCts.Cancel();
            oldCts.Dispose();
        }

        if (shouldStartLoop)
        {
            ReceiverTrace.Log($"Control plane agent enabled: {normalized.BaseUrl}");
        }
    }

    public void RestartLoop()
    {
        CancellationTokenSource? oldCts;
        CancellationTokenSource? newCts = null;
        var shouldRestart = false;
        ControlPlaneAgentConfiguration configuration;

        lock (_sync)
        {
            configuration = _configuration;
            if (!configuration.Enabled || string.IsNullOrWhiteSpace(configuration.BaseUrl))
            {
                return;
            }

            oldCts = _loopCts;
            newCts = new CancellationTokenSource();
            _loopCts = newCts;
            _loopTask = Task.Run(() => RunLoopAsync(newCts.Token));
            shouldRestart = true;
        }

        oldCts?.Cancel();
        oldCts?.Dispose();

        if (shouldRestart)
        {
            ReceiverTrace.Log($"Control plane agent restart requested: {configuration.BaseUrl}");
        }
    }

    public void Dispose()
    {
        CancellationTokenSource? cts;
        Task? loopTask;
        lock (_sync)
        {
            cts = _loopCts;
            loopTask = _loopTask;
            _loopCts = null;
            _loopTask = null;
        }

        if (cts is not null)
        {
            cts.Cancel();
            try
            {
                loopTask?.Wait(250);
            }
            catch
            {
            }
            cts.Dispose();
        }

        _httpClient.Dispose();
    }

    private async Task RunLoopAsync(CancellationToken cancellationToken)
    {
        while (!cancellationToken.IsCancellationRequested)
        {
            ControlPlaneAgentConfiguration configuration;
            ControlPlaneRegistrationState? registrationState;
            lock (_sync)
            {
                configuration = _configuration;
                registrationState = _registrationState;
            }

            if (!configuration.Enabled || string.IsNullOrWhiteSpace(configuration.BaseUrl))
            {
                return;
            }

            try
            {
                if (registrationState is null)
                {
                    registrationState = await RegisterHostAsync(configuration, cancellationToken);
                    lock (_sync)
                    {
                        _registrationState = registrationState;
                    }
                    SaveRegistrationState(configuration.BaseUrl, registrationState);
                }
                else
                {
                    var heartbeat = await SendHeartbeatAsync(configuration, registrationState, cancellationToken);
                    var lease = ResolveEffectiveLease(
                        await FetchLeaseAsync(configuration, registrationState, cancellationToken),
                        now: DateTimeOffset.UtcNow);
                    ControlPlaneAgentSnapshot updatedSnapshot;
                    lock (_sync)
                    {
                        updatedSnapshot = _snapshot with
                        {
                            Status = configuration.SenderBusy ? "Busy" : "Online",
                            HostId = registrationState.HostId,
                            LeaseStatus = lease?.StatusText ?? "No lease",
                            LeaseSessionId = lease?.SessionId ?? "-",
                            LeaseSessionToken = lease?.SessionToken ?? string.Empty,
                            LeaseClientLabel = lease?.ClientLabel ?? "-",
                            LeaseExpiresUtc = lease?.ExpiresUtc,
                            LeaseReceiverEndpoint = lease?.ReceiverEndpoint?.DisplayText ?? "-",
                            LeaseRouteKind = lease?.RouteKind ?? "-",
                            LeaseRelayEndpoint = lease?.RelayEndpoint?.DisplayText ?? "-",
                            LeaseRelayRegion = lease?.RelayRegion ?? "-",
                            LeaseProbeEndpoint = lease?.ProbeEndpoint?.DisplayText ?? "-",
                            LeaseProbeToken = lease?.ProbeToken ?? string.Empty,
                            LeaseNatStatus = lease?.NatStatus ?? "-",
                            LeaseReceiverRegistered = lease?.ReceiverRegistered ?? false,
                            LeaseHostReady = lease?.HostReady ?? false,
                            LeaseCodecPreference = lease?.CodecPreference ?? "-",
                            LeaseRequestedWidth = lease?.DesiredStream.RequestedWidth ?? 0,
                            LeaseRequestedHeight = lease?.DesiredStream.RequestedHeight ?? 0,
                            LeaseRequestedFps = lease?.DesiredStream.RequestedFps ?? 0,
                            LeaseRequestedBitrateBps = lease?.DesiredStream.RequestedBitrateBps ?? 0,
                            LeaseCaptureCursor = lease?.DesiredStream.CaptureCursor,
                            LeaseAdaptiveMode = lease?.DesiredStream.AdaptiveMode,
                            LeaseUnattendedAuthorized = lease?.UnattendedAuthorized == true,
                            HeartbeatIntervalSeconds = Math.Max(2, _snapshot.HeartbeatIntervalSeconds),
                            LastHeartbeatUtc = heartbeat.ServerUtc,
                            LastError = "-",
                        };
                        _snapshot = updatedSnapshot;
                    }
                    NotifySnapshotChanged(updatedSnapshot);

                    try
                    {
                        await MaybePublishNatProbeAsync(configuration, lease, cancellationToken);
                    }
                    catch (OperationCanceledException)
                    {
                        throw;
                    }
                    catch (Exception ex)
                    {
                        ReceiverTrace.Log(ex, "Control plane NAT probe publish failed");
                    }

                    try
                    {
                        await MaybePushTelemetryAsync(configuration, registrationState, lease, cancellationToken);
                    }
                    catch (OperationCanceledException)
                    {
                        throw;
                    }
                    catch (Exception ex)
                    {
                        ReceiverTrace.Log(ex, "Control plane telemetry publish failed");
                    }
                }
            }
            catch (OperationCanceledException)
            {
                return;
            }
            catch (HttpRequestException ex)
            {
                UpdateErrorSnapshot($"Network error: {ex.Message}");
            }
            catch (ControlPlaneRegistrationExpiredException)
            {
                ControlPlaneAgentSnapshot reRegisteringSnapshot;
                lock (_sync)
                {
                    _registrationState = null;
                    reRegisteringSnapshot = _snapshot with
                    {
                        Status = "Re-registering...",
                        HostId = "-",
                        LeaseStatus = "-",
                        LeaseSessionId = "-",
                        LeaseSessionToken = string.Empty,
                        LeaseClientLabel = "-",
                        LeaseExpiresUtc = null,
                        LeaseReceiverEndpoint = "-",
                        LeaseRouteKind = "-",
                        LeaseRelayEndpoint = "-",
                        LeaseRelayRegion = "-",
                        LeaseProbeEndpoint = "-",
                        LeaseProbeToken = string.Empty,
                        LeaseNatStatus = "-",
                        LeaseReceiverRegistered = false,
                        LeaseHostReady = false,
                        LeaseCodecPreference = "-",
                        LeaseRequestedWidth = 0,
                        LeaseRequestedHeight = 0,
                        LeaseRequestedFps = 0,
                        LeaseRequestedBitrateBps = 0,
                        LeaseCaptureCursor = null,
                        LeaseAdaptiveMode = null,
                        LeaseUnattendedAuthorized = false,
                    };
                    _snapshot = reRegisteringSnapshot;
                }
                NotifySnapshotChanged(reRegisteringSnapshot);
            }
            catch (Exception ex)
            {
                UpdateErrorSnapshot(ex.Message);
                ReceiverTrace.Log(ex, "Control plane agent loop failed");
            }

            var snapshot = GetSnapshot();
            var leaseJustCleared =
                string.IsNullOrWhiteSpace(snapshot.LeaseSessionId) ||
                string.Equals(snapshot.LeaseSessionId, "-", StringComparison.Ordinal) ||
                snapshot.LeaseStatus is "Stopped" or "Expired" or "No lease";
            var delaySeconds = leaseJustCleared
                ? 0.5
                : Math.Clamp(snapshot.HeartbeatIntervalSeconds, 2, 15);
            try
            {
                await Task.Delay(TimeSpan.FromSeconds(delaySeconds), cancellationToken);
            }
            catch (OperationCanceledException)
            {
                return;
            }
        }
    }

    private async Task<ControlPlaneRegistrationState> RegisterHostAsync(ControlPlaneAgentConfiguration configuration, CancellationToken cancellationToken)
    {
        var persisted = LoadRegistrationState(configuration.BaseUrl);
        var directAddress = ResolvePreferredIpv4Address();
        var request = new RegisterHostRequest(
            HostId: persisted?.HostId,
            HostSecret: persisted?.HostSecret,
            DisplayName: configuration.DisplayName,
            Region: configuration.Region,
            DirectAddress: directAddress,
            DirectPort: configuration.DirectPort,
            EncoderBackends: configuration.EncoderBackends,
            SupportsHevc: configuration.SupportsHevc,
            SupportsAudio: configuration.SupportsAudio,
            SupportsGamepad: configuration.SupportsGamepad,
            Capabilities: new HostCapabilitiesRequest(
                CpuModel: configuration.Capabilities.CpuModel,
                GpuModel: configuration.Capabilities.GpuModel,
                RamGb: configuration.Capabilities.RamGb,
                MaxWidth: configuration.Capabilities.MaxWidth,
                MaxHeight: configuration.Capabilities.MaxHeight,
                MaxFps: configuration.Capabilities.MaxFps,
                SupportedEncodeCodecs: configuration.Capabilities.SupportedEncodeCodecs,
                SupportedDecodeCodecs: configuration.Capabilities.SupportedDecodeCodecs,
                SupportedEncoderBackends: configuration.Capabilities.SupportedEncoderBackends,
                LanAddresses: configuration.Capabilities.LanAddresses));

        using var response = await _httpClient.PostAsJsonAsync(
            BuildUri(configuration.BaseUrl, "/api/hosts/register"),
            request,
            cancellationToken);
        response.EnsureSuccessStatusCode();

        var payload = await response.Content.ReadFromJsonAsync<RegisterHostResponse>(cancellationToken: cancellationToken);
        if (payload is null || string.IsNullOrWhiteSpace(payload.HostId) || string.IsNullOrWhiteSpace(payload.HostSecret))
        {
            throw new InvalidOperationException("Control plane returned an empty host registration response.");
        }

        var registrationState = new ControlPlaneRegistrationState(payload.HostId, payload.HostSecret);
        ControlPlaneAgentSnapshot snapshot;
        lock (_sync)
        {
            snapshot = _snapshot with
            {
                Status = configuration.SenderBusy ? "Busy" : "Online",
                HostId = payload.HostId,
                HeartbeatIntervalSeconds = Math.Clamp(payload.HeartbeatIntervalSeconds <= 0 ? 5 : payload.HeartbeatIntervalSeconds, 2, 15),
                LastHeartbeatUtc = DateTimeOffset.UtcNow,
                LastError = "-",
            };
            _snapshot = snapshot;
        }

        NotifySnapshotChanged(snapshot);

        return registrationState;
    }

    private async Task<HostHeartbeatResponse> SendHeartbeatAsync(
        ControlPlaneAgentConfiguration configuration,
        ControlPlaneRegistrationState registrationState,
        CancellationToken cancellationToken)
    {
        var request = new HostHeartbeatRequest(
            HostSecret: registrationState.HostSecret,
            CpuLoadPercent: null,
            GpuLoadPercent: null,
            NetworkKbps: null,
            Availability: configuration.SenderBusy ? 2 : 1,
            DirectAddress: ResolvePreferredIpv4Address(),
            DirectPort: configuration.DirectPort);

        using var response = await _httpClient.PostAsJsonAsync(
            BuildUri(configuration.BaseUrl, $"/api/hosts/{registrationState.HostId}/heartbeat"),
            request,
            cancellationToken);

        if (response.StatusCode is HttpStatusCode.NotFound or HttpStatusCode.Unauthorized)
        {
            ClearPersistedRegistration();
            throw new ControlPlaneRegistrationExpiredException();
        }

        response.EnsureSuccessStatusCode();

        var payload = await response.Content.ReadFromJsonAsync<HostHeartbeatResponse>(cancellationToken: cancellationToken);
        if (payload is null)
        {
            throw new InvalidOperationException("Control plane returned an empty heartbeat response.");
        }

        return payload;
    }

    private async Task<HostLeaseResponse?> FetchLeaseAsync(
        ControlPlaneAgentConfiguration configuration,
        ControlPlaneRegistrationState registrationState,
        CancellationToken cancellationToken)
    {
        using var response = await _httpClient.GetAsync(
            BuildUri(
                configuration.BaseUrl,
                $"/api/hosts/{registrationState.HostId}/lease?hostSecret={Uri.EscapeDataString(registrationState.HostSecret)}"),
            cancellationToken);

        if (response.StatusCode == HttpStatusCode.NoContent)
        {
            LogLeasePollState("no_content", null);
            return null;
        }

        if (response.StatusCode is HttpStatusCode.NotFound or HttpStatusCode.Unauthorized)
        {
            ClearPersistedRegistration();
            throw new ControlPlaneRegistrationExpiredException();
        }

        response.EnsureSuccessStatusCode();
        var lease = await response.Content.ReadFromJsonAsync<HostLeaseResponse>(cancellationToken: cancellationToken);
        LogLeasePollState("content", lease);
        return lease;
    }

    private void UpdateErrorSnapshot(string message)
    {
        ControlPlaneAgentSnapshot snapshot;
        lock (_sync)
        {
            snapshot = _snapshot with
            {
                Status = "Error",
                LastError = message,
            };
            _snapshot = snapshot;
        }
        NotifySnapshotChanged(snapshot);
    }

    private HostLeaseResponse? ResolveEffectiveLease(HostLeaseResponse? fetchedLease, DateTimeOffset now)
    {
        if (fetchedLease is not null)
        {
            _lastEffectiveLease = fetchedLease;
            _lastEffectiveLeaseUtc = now;
            _lastLeaseGapSessionId = null;
            if (!string.Equals(_lastLoggedLeaseSessionId, fetchedLease.SessionId, StringComparison.Ordinal))
            {
                ReceiverTrace.Log(
                    $"Control plane lease active: session={fetchedLease.SessionId}; " +
                    $"receiver={fetchedLease.ReceiverEndpoint?.DisplayText ?? "-"}; " +
                    $"status={fetchedLease.StatusText}; route={fetchedLease.RouteKind}.");
                _lastLoggedLeaseSessionId = fetchedLease.SessionId;
            }

            return fetchedLease;
        }

        if (_lastEffectiveLease is not null)
        {
            ReceiverTrace.Log($"Control plane lease cleared for {_lastEffectiveLease.SessionId}.");
        }

        _lastEffectiveLease = null;
        _lastEffectiveLeaseUtc = DateTimeOffset.MinValue;
        _lastLoggedLeaseSessionId = null;
        _lastLeaseGapSessionId = null;
        return null;
    }

    private void NotifySnapshotChanged(ControlPlaneAgentSnapshot snapshot)
    {
        LogSnapshotLeaseState(snapshot);
        try
        {
            SnapshotChanged?.Invoke(snapshot);
        }
        catch
        {
        }
    }

    private async Task MaybePushTelemetryAsync(
        ControlPlaneAgentConfiguration configuration,
        ControlPlaneRegistrationState registrationState,
        HostLeaseResponse? lease,
        CancellationToken cancellationToken)
    {
        var now = DateTimeOffset.UtcNow;
        if (now - _lastTelemetryUtc < TimeSpan.FromSeconds(15))
        {
            return;
        }

        if (!configuration.SenderBusy && lease is null)
        {
            return;
        }

        var payload = new Dictionary<string, object?>
        {
            ["senderBusy"] = configuration.SenderBusy,
            ["encoderPath"] = configuration.EncoderPath,
            ["codec"] = configuration.Codec,
            ["resolution"] = configuration.Resolution,
            ["captureFps"] = configuration.CaptureFps,
            ["encodeFps"] = configuration.EncodeFps,
            ["receiverDecodeFps"] = configuration.ReceiverDecodeFps,
            ["pulseEstimateMs"] = configuration.PulseEstimateMs,
            ["inputEstimateMs"] = configuration.InputEstimateMs,
            ["framesDropped"] = configuration.FramesDropped,
            ["packetsSent"] = configuration.PacketsSent,
            ["leaseStatus"] = lease?.StatusText,
            ["natStatus"] = lease?.NatStatus,
        };

        var request = new TelemetryIngestRequest(
            HostId: registrationState.HostId,
            SessionId: lease?.SessionId,
            SessionToken: lease?.SessionToken,
            Source: "receiver-native-host-agent",
            EventType: "sender_snapshot",
            Payload: payload);

        using var response = await _httpClient.PostAsJsonAsync(
            BuildUri(configuration.BaseUrl, "/api/telemetry/session"),
            request,
            cancellationToken);
        response.EnsureSuccessStatusCode();
        _lastTelemetryUtc = now;
    }

    private async Task MaybePublishNatProbeAsync(
        ControlPlaneAgentConfiguration configuration,
        HostLeaseResponse? lease,
        CancellationToken cancellationToken)
    {
        if (lease is null ||
            string.IsNullOrWhiteSpace(lease.SessionId) ||
            string.IsNullOrWhiteSpace(lease.SessionToken) ||
            lease.ProbeEndpoint is null ||
            string.IsNullOrWhiteSpace(lease.ProbeToken))
        {
            return;
        }

        var now = DateTimeOffset.UtcNow;
        if (now - _lastNatProbeUtc < TimeSpan.FromSeconds(20))
        {
            return;
        }

        var observed = await RunNatProbeAsync(
            lease.SessionId,
            lease.ProbeToken,
            lease.ProbeEndpoint.Host,
            lease.ProbeEndpoint.Port,
            "host",
            cancellationToken);
        if (observed is null)
        {
            return;
        }

        var request = new SessionNatProbeRequest(
            SessionToken: lease.SessionToken,
            ProbeToken: lease.ProbeToken,
            Role: "host",
            ObservedAddress: observed.ObservedAddress,
            ObservedPort: observed.ObservedPort,
            LocalAddress: observed.LocalAddress,
            LocalPort: observed.LocalPort,
            NetworkType: "udp");

        using var response = await _httpClient.PostAsJsonAsync(
            BuildUri(configuration.BaseUrl, $"/api/sessions/{lease.SessionId}/nat/probe"),
            request,
            cancellationToken);
        response.EnsureSuccessStatusCode();
        _lastNatProbeUtc = now;
    }

    private void LogLeasePollState(string kind, HostLeaseResponse? lease)
    {
        var state =
            $"{kind}|session={lease?.SessionId ?? "-"}|status={lease?.StatusText ?? "No lease"}|receiver={lease?.ReceiverEndpoint?.DisplayText ?? "-"}|route={lease?.RouteKind ?? "-"}|ready={lease?.HostReady ?? false}|registered={lease?.ReceiverRegistered ?? false}";
        if (string.Equals(_lastLoggedLeasePollState, state, StringComparison.Ordinal))
        {
            return;
        }

        ReceiverTrace.Log(
            $"Control plane lease poll: kind={kind}; session={lease?.SessionId ?? "-"}; status={lease?.StatusText ?? "No lease"}; " +
            $"receiver={lease?.ReceiverEndpoint?.DisplayText ?? "-"}; route={lease?.RouteKind ?? "-"}; " +
            $"registered={lease?.ReceiverRegistered ?? false}; ready={lease?.HostReady ?? false}.");
        _lastLoggedLeasePollState = state;
    }

    private void LogSnapshotLeaseState(ControlPlaneAgentSnapshot snapshot)
    {
        var state =
            $"status={snapshot.LeaseStatus}|session={snapshot.LeaseSessionId}|receiver={snapshot.LeaseReceiverEndpoint}|route={snapshot.LeaseRouteKind}|registered={snapshot.LeaseReceiverRegistered}|ready={snapshot.LeaseHostReady}";
        if (string.Equals(_lastLoggedSnapshotLeaseState, state, StringComparison.Ordinal))
        {
            return;
        }

        ReceiverTrace.Log(
            $"Control plane snapshot: status={snapshot.LeaseStatus}; session={snapshot.LeaseSessionId}; " +
            $"receiver={snapshot.LeaseReceiverEndpoint}; route={snapshot.LeaseRouteKind}; " +
            $"registered={snapshot.LeaseReceiverRegistered}; ready={snapshot.LeaseHostReady}.");
        _lastLoggedSnapshotLeaseState = state;
    }

    private static async Task<NatProbeEcho?> RunNatProbeAsync(
        string sessionId,
        string probeToken,
        string probeHost,
        int probePort,
        string role,
        CancellationToken cancellationToken)
    {
        using var udpClient = new UdpClient(AddressFamily.InterNetwork);
        udpClient.Client.ReceiveTimeout = 2000;
        var payload = JsonSerializer.SerializeToUtf8Bytes(new NatProbeWireRequest(
            Kind: "nat_probe",
            SessionId: sessionId,
            ProbeToken: probeToken,
            Role: role));

        await udpClient.SendAsync(payload, payload.Length, probeHost, probePort);
        UdpReceiveResult response;
        try
        {
            response = await udpClient.ReceiveAsync(cancellationToken);
        }
        catch
        {
            return null;
        }

        var ack = JsonSerializer.Deserialize<NatProbeWireResponse>(response.Buffer);
        if (ack is null ||
            !string.Equals(ack.Kind, "nat_probe_ack", StringComparison.OrdinalIgnoreCase) ||
            !string.Equals(ack.SessionId, sessionId, StringComparison.Ordinal) ||
            !string.Equals(ack.ProbeToken, probeToken, StringComparison.Ordinal) ||
            string.IsNullOrWhiteSpace(ack.ObservedAddress) ||
            ack.ObservedPort is < 1 or > 65535)
        {
            return null;
        }

        var local = udpClient.Client.LocalEndPoint as IPEndPoint;
        return new NatProbeEcho(
            ObservedAddress: ack.ObservedAddress.Trim(),
            ObservedPort: ack.ObservedPort,
            LocalAddress: local?.Address.ToString(),
            LocalPort: local?.Port);
    }

    private ControlPlaneRegistrationState? LoadRegistrationState(string baseUrl)
    {
        try
        {
            if (!File.Exists(_statePath))
            {
                return null;
            }

            var payload = JsonSerializer.Deserialize<PersistedControlPlaneState>(File.ReadAllText(_statePath), _jsonOptions);
            if (payload is null || !string.Equals(NormalizeBaseUrl(payload.BaseUrl), baseUrl, StringComparison.OrdinalIgnoreCase))
            {
                return null;
            }

            if (string.IsNullOrWhiteSpace(payload.HostId) || string.IsNullOrWhiteSpace(payload.HostSecret))
            {
                return null;
            }

            return new ControlPlaneRegistrationState(payload.HostId, payload.HostSecret);
        }
        catch (Exception ex)
        {
            ReceiverTrace.Log(ex, "Failed to load persisted control plane registration");
            return null;
        }
    }

    private void SaveRegistrationState(string baseUrl, ControlPlaneRegistrationState state)
    {
        try
        {
            var payload = new PersistedControlPlaneState(baseUrl, state.HostId, state.HostSecret);
            File.WriteAllText(_statePath, JsonSerializer.Serialize(payload, _jsonOptions));
        }
        catch (Exception ex)
        {
            ReceiverTrace.Log(ex, "Failed to persist control plane registration");
        }
    }

    private void ClearPersistedRegistration()
    {
        try
        {
            if (File.Exists(_statePath))
            {
                File.Delete(_statePath);
            }
        }
        catch (Exception ex)
        {
            ReceiverTrace.Log(ex, "Failed to clear persisted control plane registration");
        }
    }

    private static Uri BuildUri(string baseUrl, string relativePath) =>
        new($"{NormalizeBaseUrl(baseUrl)}{relativePath}", UriKind.Absolute);

    private static string NormalizeBaseUrl(string? baseUrl) =>
        string.IsNullOrWhiteSpace(baseUrl) ? string.Empty : baseUrl.Trim().TrimEnd('/');

    private static string ResolvePreferredIpv4Address()
    {
        foreach (var networkInterface in NetworkInterface.GetAllNetworkInterfaces())
        {
            if (networkInterface.OperationalStatus != OperationalStatus.Up ||
                networkInterface.NetworkInterfaceType == NetworkInterfaceType.Loopback)
            {
                continue;
            }

            IPInterfaceProperties? properties;
            try
            {
                properties = networkInterface.GetIPProperties();
            }
            catch
            {
                continue;
            }

            foreach (var unicast in properties.UnicastAddresses)
            {
                if (unicast.Address.AddressFamily == AddressFamily.InterNetwork && !IPAddress.IsLoopback(unicast.Address))
                {
                    return unicast.Address.ToString();
                }
            }
        }

        return string.Empty;
    }

    private sealed record ControlPlaneRegistrationState(string HostId, string HostSecret);

    private sealed record PersistedControlPlaneState(string BaseUrl, string HostId, string HostSecret);

    private sealed record RegisterHostRequest(
        string? HostId,
        string? HostSecret,
        string DisplayName,
        string Region,
        string? DirectAddress,
        int DirectPort,
        string[] EncoderBackends,
        bool SupportsHevc,
        bool SupportsAudio,
        bool SupportsGamepad,
        HostCapabilitiesRequest Capabilities);

    private sealed record HostCapabilitiesRequest(
        string? CpuModel,
        string? GpuModel,
        int RamGb,
        int MaxWidth,
        int MaxHeight,
        int MaxFps,
        string[]? SupportedEncodeCodecs,
        string[]? SupportedDecodeCodecs,
        string[]? SupportedEncoderBackends,
        string[]? LanAddresses);

    private sealed record RegisterHostResponse(
        string HostId,
        string HostSecret,
        int HeartbeatIntervalSeconds);

    private sealed record HostHeartbeatRequest(
        string HostSecret,
        double? CpuLoadPercent,
        double? GpuLoadPercent,
        double? NetworkKbps,
        int Availability,
        string? DirectAddress,
        int DirectPort);

    private sealed record HostHeartbeatResponse(
        string HostId,
        int Availability,
        bool Online,
        string? ActiveSessionId,
        DateTimeOffset ServerUtc);

    private sealed record HostLeaseResponse(
        string HostId,
        string SessionId,
        string SessionToken,
        string ClientLabel,
        int Status,
        StreamEndpoint StreamEndpoint,
        StreamEndpoint? ReceiverEndpoint,
        string RouteKind,
        StreamEndpoint? RelayEndpoint,
        string? RelayRegion,
        StreamEndpoint? ProbeEndpoint,
        string ProbeToken,
        string NatStatus,
        bool ReceiverRegistered,
        bool HostReady,
        NatProbeObservation? HostNatProbe,
        NatProbeObservation? ClientNatProbe,
        DesiredStreamSettings DesiredStream,
        bool UnattendedAuthorized,
        string? CodecPreference,
        bool AudioRequested,
        int ControllerCount,
        DateTimeOffset CreatedUtc,
        DateTimeOffset UpdatedUtc,
        DateTimeOffset ExpiresUtc)
    {
        public string StatusText => Status switch
        {
            0 => "Pending",
            1 => "Active",
            2 => "Stopped",
            3 => "Expired",
            _ => $"State {Status}",
        };
    }

    private sealed record StreamEndpoint(
        string Host,
        int Port,
        string Transport)
    {
        public string DisplayText => $"{Host}:{Port} ({Transport})";
    }

    private sealed record DesiredStreamSettings(
        int? RequestedWidth,
        int? RequestedHeight,
        int? RequestedFps,
        int? RequestedBitrateBps,
        bool? CaptureCursor,
        bool? AdaptiveMode);

    private sealed record NatProbeObservation(
        string ObservedAddress,
        int ObservedPort,
        string? LocalAddress,
        int? LocalPort,
        string? NetworkType,
        DateTimeOffset ReportedUtc);

    private sealed record SessionNatProbeRequest(
        string SessionToken,
        string ProbeToken,
        string Role,
        string ObservedAddress,
        int ObservedPort,
        string? LocalAddress,
        int? LocalPort,
        string? NetworkType);

    private sealed record NatProbeWireRequest(
        string Kind,
        string SessionId,
        string ProbeToken,
        string Role);

    private sealed record NatProbeWireResponse(
        string Kind,
        string SessionId,
        string ProbeToken,
        string ObservedAddress,
        int ObservedPort);

    private sealed record NatProbeEcho(
        string ObservedAddress,
        int ObservedPort,
        string? LocalAddress,
        int? LocalPort);

    private sealed record TelemetryIngestRequest(
        string? HostId,
        string? SessionId,
        string? SessionToken,
        string? Source,
        string? EventType,
        Dictionary<string, object?> Payload);

    private sealed class ControlPlaneRegistrationExpiredException : Exception;
}
