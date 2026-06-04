namespace ReceiverNative;

using System.Globalization;
using System.Net;
using System.Net.Http;
using System.Net.Http.Headers;
using System.Net.Http.Json;
using System.Net.Sockets;
using System.Text.Json;
using System.Text.Json.Serialization;

internal sealed class HostAvailabilityJsonConverter : JsonConverter<string>
{
    public override string Read(ref Utf8JsonReader reader, Type typeToConvert, JsonSerializerOptions options)
    {
        return reader.TokenType switch
        {
            JsonTokenType.String => reader.GetString() ?? string.Empty,
            JsonTokenType.Number => MapAvailability(reader.GetInt32()),
            _ => throw new JsonException($"Unsupported availability token: {reader.TokenType}"),
        };
    }

    public override void Write(Utf8JsonWriter writer, string value, JsonSerializerOptions options)
    {
        writer.WriteStringValue(value);
    }

    private static string MapAvailability(int value) => value switch
    {
        0 => "Offline",
        1 => "Online",
        2 => "Busy",
        3 => "Disabled",
        _ => value.ToString(CultureInfo.InvariantCulture),
    };
}

internal sealed class SessionStatusJsonConverter : JsonConverter<string>
{
    public override string Read(ref Utf8JsonReader reader, Type typeToConvert, JsonSerializerOptions options)
    {
        return reader.TokenType switch
        {
            JsonTokenType.String => reader.GetString() ?? string.Empty,
            JsonTokenType.Number => MapStatus(reader.GetInt32()),
            _ => throw new JsonException($"Unsupported session status token: {reader.TokenType}"),
        };
    }

    public override void Write(Utf8JsonWriter writer, string value, JsonSerializerOptions options)
    {
        writer.WriteStringValue(value);
    }

    private static string MapStatus(int value) => value switch
    {
        0 => "Pending",
        1 => "Active",
        2 => "Stopped",
        3 => "Expired",
        _ => value.ToString(CultureInfo.InvariantCulture),
    };
}

internal sealed class ControlPlaneApiException : InvalidOperationException
{
    public ControlPlaneApiException(string code, string message)
        : base(message)
    {
        Code = code;
    }

    public string Code { get; }
}

internal sealed record DesktopControlPlaneHostSummary(
    string HostId,
    string HostCode,
    string DisplayName,
    string Region,
    bool Online,
    [property: JsonConverter(typeof(HostAvailabilityJsonConverter))] string Availability,
    string? ActiveSessionId,
    bool SupportsHevc,
    bool SupportsAudio,
    bool SupportsGamepad,
    decimal? PricePerHour = null,
    string? Currency = null,
    string? Description = null)
{
    public string UiLabel =>
        PricePerHour is > 0 && !string.IsNullOrWhiteSpace(Currency)
            ? $"{DisplayName} [{HostCode}] [{Region}] {PricePerHour:0.##} {Currency}/h {ToUiAvailability()}"
            : $"{DisplayName} [{HostCode}] [{Region}] {ToUiAvailability()}";

    private string ToUiAvailability()
    {

        if (!Online)
        {
            return "Оффлайн";
        }

        return !string.IsNullOrWhiteSpace(ActiveSessionId)
            ? "Занят"
            : "Можно подключиться";
    }
}

internal sealed record DesktopControlPlaneDesiredStreamRequest(
    int? Width,
    int? Height,
    int? Fps,
    int? BitrateBps,
    bool? CaptureCursor,
    bool? AdaptiveMode,
    IReadOnlyList<string>? PreferredCodecs = null,
    string? PresetId = null);

internal sealed record DesktopControlPlaneClientCapabilities(
    IReadOnlyList<string>? SupportedDecodeCodecs = null,
    IReadOnlyList<string>? LanAddresses = null);

internal sealed record DesktopControlPlaneSessionLease(
    string SessionId,
    string SessionToken,
    string HostId,
    string HostDisplayName,
    string Status,
    string RouteKind,
    string RouteState,
    string SessionHealth,
    string SessionHealthReason,
    string RouteActionHint,
    string RouteActionReason,
    int RouteFallbackReadyDurationSeconds,
    int RouteRecoveryReadyDurationSeconds,
    int RecommendedSyncDelaySeconds,
    string TransportLossLevel,
    string TransportAnomalyKind,
    string TransportAnomalyReason,
    string TransportAnomalyConfidence,
    int ReceiverTelemetryAgeSeconds,
    int SenderTelemetryAgeSeconds,
    int RouteRecoveryCount,
    int RouteRecoveryCooldownSeconds,
    int RouteFallbackCount,
    int RouteFallbackCooldownSeconds,
    string? CodecPreference,
    int RouteVersion,
    string? RelayAddress,
    int? RelayPort,
    string? RelayRegion,
    string? ProbeAddress,
    int? ProbePort,
    string ProbeToken,
    string NatStatus,
    int HostNatProbeAgeSeconds,
    int ClientNatProbeAgeSeconds,
    bool NatProbeFresh,
    string? ReceiverAddress,
    int? ReceiverPort,
    string? LastRouteActionKind = null,
    string? LastRouteActionReason = null,
    string? LastRouteActionActor = null,
    DateTimeOffset? LastRouteActionUtc = null);

internal sealed record DesktopControlPlaneConnectInstructions(
    string SessionId,
    string HostId,
    string HostDisplayName,
    string Status,
    string RouteKind,
    string RouteState,
    int RouteVersion,
    string SessionHealth,
    string SessionHealthReason,
    string RouteActionHint,
    string RouteActionReason,
    int RouteFallbackReadyDurationSeconds,
    int RouteRecoveryReadyDurationSeconds,
    int RecommendedSyncDelaySeconds,
    string TransportLossLevel,
    string TransportAnomalyKind,
    string TransportAnomalyReason,
    string TransportAnomalyConfidence,
    int ReceiverTelemetryAgeSeconds,
    int SenderTelemetryAgeSeconds,
    int RouteRecoveryCount,
    int RouteRecoveryCooldownSeconds,
    int RouteFallbackCount,
    int RouteFallbackCooldownSeconds,
    string StreamHost,
    int StreamPort,
    string? RelayHost,
    int? RelayPort,
    string? RelayRegion,
    string? ProbeHost,
    int? ProbePort,
    string ProbeToken,
    string NatStatus,
    int HostNatProbeAgeSeconds,
    int ClientNatProbeAgeSeconds,
    bool NatProbeFresh,
    string? ReceiverHost,
    int? ReceiverPort,
    string? LastRouteActionKind = null,
    string? LastRouteActionReason = null,
    string? LastRouteActionActor = null,
    DateTimeOffset? LastRouteActionUtc = null);

internal sealed record DesktopControlPlaneRoutePolicy(
    string SessionId,
    string HostId,
    string RouteKind,
    string RouteState,
    int RouteVersion,
    string SessionHealth,
    string SessionHealthReason,
    string RouteActionHint,
    string RouteActionReason,
    int RecommendedSyncDelaySeconds,
    string TransportLossLevel,
    string TransportAnomalyKind,
    string TransportAnomalyReason,
    string TransportAnomalyConfidence,
    bool ActionableAnomaly,
    bool HighConfidenceAnomaly,
    int FallbackWarmupSeconds,
    int FallbackReadyDurationSeconds,
    bool FallbackReady,
    int RecoveryWarmupSeconds,
    int RecoveryReadyDurationSeconds,
    bool RecoveryReady,
    int FallbackCooldownSeconds,
    int RecoveryCooldownSeconds,
    int ReceiverTelemetryAgeSeconds,
    int SenderTelemetryAgeSeconds,
    string NatStatus,
    int HostNatProbeAgeSeconds,
    int ClientNatProbeAgeSeconds,
    bool NatProbeFresh);

internal sealed record DesktopControlPlaneAuthState(
    string Mode,
    string Label,
    bool UserAuthenticated);

internal sealed record DesktopControlPlaneManagedSessionState(
    string BaseUrl,
    string SessionId,
    string SessionToken,
    string HostId,
    string HostDisplayName,
    string RouteKind,
    string RouteState,
    string SessionHealth,
    string SessionHealthReason,
    string RouteActionHint,
    string RouteActionReason,
    int RouteFallbackReadyDurationSeconds,
    int RouteRecoveryReadyDurationSeconds,
    int RecommendedSyncDelaySeconds,
    string TransportLossLevel,
    string TransportAnomalyKind,
    string TransportAnomalyReason,
    string TransportAnomalyConfidence,
    int ReceiverTelemetryAgeSeconds,
    int SenderTelemetryAgeSeconds,
    int RouteRecoveryCount,
    int RouteRecoveryCooldownSeconds,
    string NatStatus,
    int HostNatProbeAgeSeconds,
    int ClientNatProbeAgeSeconds,
    bool NatProbeFresh,
    string? RelayAddress,
    int? RelayPort,
    string? ReceiverAddress,
    int? ReceiverPort,
    string? ProbeAddress,
    int? ProbePort,
    string ProbeToken,
    int RouteVersion = 0,
    int RouteFallbackCount = 0,
    int RouteFallbackCooldownSeconds = 0,
    string? LastRouteActionKind = null,
    string? LastRouteActionReason = null,
    string? LastRouteActionActor = null,
    DateTimeOffset? LastRouteActionUtc = null,
    string? CodecPreference = null);

internal sealed class DesktopControlPlaneClient : IDisposable
{
    private readonly HttpClient _httpClient = new() { Timeout = TimeSpan.FromSeconds(5) };
    private readonly JsonSerializerOptions _jsonOptions = new(JsonSerializerDefaults.Web);
    private readonly string _statePath;

    private DesktopControlPlanePersistedState? _persistedState;
    private DesktopControlPlaneAccessSession? _accessSession;

    public DesktopControlPlaneClient()
    {
        var directory = Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
            "EvertyNativeReceiver");
        Directory.CreateDirectory(directory);
        _statePath = Path.Combine(directory, "control-plane-client.json");
    }

    public void Dispose() => _httpClient.Dispose();

    public DesktopControlPlaneAuthState GetAuthState()
    {
        _persistedState ??= LoadPersistedState();
        if (!string.IsNullOrWhiteSpace(_persistedState?.UserEmail))
        {
            return new DesktopControlPlaneAuthState("user", _persistedState.UserEmail!, UserAuthenticated: true);
        }

        if (!string.IsNullOrWhiteSpace(_persistedState?.DeviceId))
        {
            return new DesktopControlPlaneAuthState("device", _persistedState.DeviceId!, UserAuthenticated: false);
        }

        return new DesktopControlPlaneAuthState("anonymous", "-", UserAuthenticated: false);
    }

    public DesktopControlPlaneManagedSessionState? GetManagedSessionState(string baseUrl)
    {
        _persistedState ??= LoadPersistedState();
        if (_persistedState is null)
        {
            return null;
        }

        if (!string.Equals(_persistedState.BaseUrl, NormalizeBaseUrl(baseUrl), StringComparison.OrdinalIgnoreCase))
        {
            return null;
        }

        return _persistedState.ManagedSession;
    }

    public void SaveManagedSessionState(DesktopControlPlaneManagedSessionState managedSession)
    {
        _persistedState ??= LoadPersistedState();
        _persistedState = (_persistedState ?? new DesktopControlPlanePersistedState(
            BaseUrl: managedSession.BaseUrl,
            DeviceId: string.Empty,
            DeviceSecret: string.Empty,
            RefreshToken: string.Empty,
            RefreshExpiresUtc: DateTimeOffset.MinValue,
            UserEmail: null,
            UserRefreshToken: null,
            UserRefreshExpiresUtc: null,
            ManagedSession: null)) with
        {
            BaseUrl = managedSession.BaseUrl,
            ManagedSession = managedSession,
        };
        SavePersistedState(_persistedState);
    }

    public void ClearManagedSessionState(string baseUrl)
    {
        _persistedState ??= LoadPersistedState();
        if (_persistedState is null ||
            !string.Equals(_persistedState.BaseUrl, NormalizeBaseUrl(baseUrl), StringComparison.OrdinalIgnoreCase))
        {
            return;
        }

        _persistedState = _persistedState with { ManagedSession = null };
        SavePersistedState(_persistedState);
    }

    public async Task<DesktopControlPlaneAuthState> RegisterUserAsync(string baseUrl, string email, string password, CancellationToken cancellationToken = default)
    {
        return await AuthenticateUserAsync(baseUrl, "/api/auth/users/register", email, password, cancellationToken);
    }

    public async Task<DesktopControlPlaneAuthState> LoginUserAsync(string baseUrl, string email, string password, CancellationToken cancellationToken = default)
    {
        return await AuthenticateUserAsync(baseUrl, "/api/auth/users/login", email, password, cancellationToken);
    }

    public async Task<IReadOnlyList<DesktopControlPlaneHostSummary>> ListHostsAsync(string baseUrl, CancellationToken cancellationToken = default)
    {
        var accessToken = await EnsureAccessTokenAsync(baseUrl, cancellationToken);
        return await ListHostsFromPathAsync(baseUrl, "/api/hosts", accessToken, cancellationToken);
    }

    private async Task<IReadOnlyList<DesktopControlPlaneHostSummary>> ListHostsFromPathAsync(
        string baseUrl,
        string path,
        string accessToken,
        CancellationToken cancellationToken)
    {
        using var request = CreateAuthorizedRequest(HttpMethod.Get, baseUrl, path, accessToken);
        using var response = await _httpClient.SendAsync(request, cancellationToken);
        await EnsureSuccessAsync(response, cancellationToken);
        var payload = await response.Content.ReadFromJsonAsync<DesktopControlPlaneHostSummary[]>(_jsonOptions, cancellationToken);
        return payload ?? Array.Empty<DesktopControlPlaneHostSummary>();
    }

    public async Task<DesktopControlPlaneSessionLease> CreateSessionAsync(
        string baseUrl,
        string hostId,
        string clientLabel,
        string clientRegion,
        string? codecPreference,
        bool preferRelay,
        bool audioRequested,
        int controllerCount,
        int leaseMinutes,
        string receiverAddress,
        int receiverPort,
        DesktopControlPlaneDesiredStreamRequest desiredStream,
        DesktopControlPlaneClientCapabilities? clientCapabilities = null,
        CancellationToken cancellationToken = default)
    {
        var accessToken = await EnsureAccessTokenAsync(baseUrl, cancellationToken);
        var requestBody = new CreateSessionRequest(
            HostId: hostId,
            ClientLabel: clientLabel,
            ClientRegion: clientRegion,
            CodecPreference: codecPreference,
            PreferredCodecs: desiredStream.PreferredCodecs?.ToArray(),
            PresetId: desiredStream.PresetId,
            PreferRelay: preferRelay,
            ReplaceExistingActorSession: true,
            AudioRequested: audioRequested,
            ControllerCount: controllerCount,
            LeaseMinutes: leaseMinutes,
            ReceiverAddress: receiverAddress,
            ReceiverPort: receiverPort,
            RequestedWidth: desiredStream.Width ?? 0,
            RequestedHeight: desiredStream.Height ?? 0,
            RequestedFps: desiredStream.Fps ?? 0,
            RequestedBitrateBps: desiredStream.BitrateBps ?? 0,
            CaptureCursor: desiredStream.CaptureCursor,
            AdaptiveMode: desiredStream.AdaptiveMode,
            Capabilities: clientCapabilities is null
                ? null
                : new ClientCapabilitiesRequest(
                    SupportedDecodeCodecs: clientCapabilities.SupportedDecodeCodecs?.ToArray(),
                    LanAddresses: clientCapabilities.LanAddresses?.ToArray()));

        using var request = CreateAuthorizedRequest(HttpMethod.Post, baseUrl, "/api/sessions", accessToken);
        request.Content = JsonContent.Create(requestBody, options: _jsonOptions);

        using var response = await _httpClient.SendAsync(request, cancellationToken);
        await EnsureSuccessAsync(response, cancellationToken);
        var payload = await response.Content.ReadFromJsonAsync<SessionLeaseResponse>(_jsonOptions, cancellationToken)
            ?? throw new InvalidOperationException("Control plane returned an empty session response.");

        return new DesktopControlPlaneSessionLease(
            SessionId: payload.SessionId,
            SessionToken: payload.SessionToken,
            HostId: payload.HostId,
            HostDisplayName: payload.HostDisplayName,
            Status: payload.Status,
            RouteKind: payload.RouteKind,
            RouteState: payload.RouteState,
            RouteVersion: payload.RouteVersion,
            SessionHealth: payload.SessionHealth,
            SessionHealthReason: payload.SessionHealthReason,
            RouteActionHint: payload.RouteActionHint,
            RouteActionReason: payload.RouteActionReason,
            RouteFallbackReadyDurationSeconds: payload.RouteFallbackReadyDurationSeconds,
            RouteRecoveryReadyDurationSeconds: payload.RouteRecoveryReadyDurationSeconds,
            RecommendedSyncDelaySeconds: payload.RecommendedSyncDelaySeconds,
            TransportLossLevel: payload.TransportLossLevel,
            TransportAnomalyKind: payload.TransportAnomalyKind,
            TransportAnomalyReason: payload.TransportAnomalyReason,
            TransportAnomalyConfidence: payload.TransportAnomalyConfidence,
            ReceiverTelemetryAgeSeconds: payload.ReceiverTelemetryAgeSeconds,
            SenderTelemetryAgeSeconds: payload.SenderTelemetryAgeSeconds,
            LastRouteActionKind: payload.LastRouteActionKind,
            LastRouteActionReason: payload.LastRouteActionReason,
            LastRouteActionActor: payload.LastRouteActionActor,
            LastRouteActionUtc: payload.LastRouteActionUtc,
            RouteRecoveryCount: payload.RouteRecoveryCount,
            RouteRecoveryCooldownSeconds: payload.RouteRecoveryCooldownSeconds,
            RouteFallbackCount: payload.RouteFallbackCount,
            RouteFallbackCooldownSeconds: payload.RouteFallbackCooldownSeconds,
            CodecPreference: payload.CodecPreference,
            RelayAddress: payload.RelayEndpoint?.Host,
            RelayPort: payload.RelayEndpoint?.Port,
            RelayRegion: payload.RelayRegion,
            ProbeAddress: payload.ProbeEndpoint?.Host,
            ProbePort: payload.ProbeEndpoint?.Port,
            ProbeToken: payload.ProbeToken,
            NatStatus: payload.NatStatus,
            HostNatProbeAgeSeconds: payload.HostNatProbeAgeSeconds,
            ClientNatProbeAgeSeconds: payload.ClientNatProbeAgeSeconds,
            NatProbeFresh: payload.NatProbeFresh,
            ReceiverAddress: payload.ReceiverEndpoint?.Host,
            ReceiverPort: payload.ReceiverEndpoint?.Port);
    }

    public async Task<DesktopControlPlaneConnectInstructions> ResumeManagedSessionAsync(
        string baseUrl,
        string sessionId,
        string sessionToken,
        CancellationToken cancellationToken = default)
    {
        await ActivateSessionAsync(baseUrl, sessionId, sessionToken, cancellationToken);
        return await GetConnectInstructionsAsync(baseUrl, sessionId, sessionToken, cancellationToken);
    }

    public async Task KeepAliveSessionAsync(
        string baseUrl,
        string sessionId,
        string sessionToken,
        CancellationToken cancellationToken = default)
    {
        var accessToken = await EnsureAccessTokenAsync(baseUrl, cancellationToken);
        using var request = CreateAuthorizedRequest(HttpMethod.Post, baseUrl, $"/api/sessions/{sessionId}/keepalive", accessToken);
        request.Content = JsonContent.Create(new SessionActionRequest(SessionToken: sessionToken, Reason: "managed_sync"), options: _jsonOptions);

        using var response = await _httpClient.SendAsync(request, cancellationToken);
        await EnsureSuccessAsync(response, cancellationToken);
    }

    public async Task<DesktopControlPlaneConnectInstructions> FallbackManagedSessionRouteAsync(
        string baseUrl,
        string sessionId,
        string sessionToken,
        string reason = "managed_sync_failure",
        CancellationToken cancellationToken = default)
    {
        var accessToken = await EnsureAccessTokenAsync(baseUrl, cancellationToken);
        using var request = CreateAuthorizedRequest(HttpMethod.Post, baseUrl, $"/api/sessions/{sessionId}/route/fallback", accessToken);
        request.Content = JsonContent.Create(new SessionActionRequest(SessionToken: sessionToken, Reason: reason), options: _jsonOptions);

        using var response = await _httpClient.SendAsync(request, cancellationToken);
        await EnsureSuccessAsync(response, cancellationToken);
        var payload = await response.Content.ReadFromJsonAsync<SessionConnectInstructionsResponse>(_jsonOptions, cancellationToken)
            ?? throw new InvalidOperationException("Control plane returned empty fallback instructions.");

        return new DesktopControlPlaneConnectInstructions(
            SessionId: payload.SessionId,
            HostId: payload.HostId,
            HostDisplayName: payload.HostDisplayName,
            Status: payload.Status,
            RouteKind: payload.RouteKind,
            RouteState: payload.RouteState,
            RouteVersion: payload.RouteVersion,
            SessionHealth: payload.SessionHealth,
            SessionHealthReason: payload.SessionHealthReason,
            RouteActionHint: payload.RouteActionHint,
            RouteActionReason: payload.RouteActionReason,
            RouteFallbackReadyDurationSeconds: payload.RouteFallbackReadyDurationSeconds,
            RouteRecoveryReadyDurationSeconds: payload.RouteRecoveryReadyDurationSeconds,
            RecommendedSyncDelaySeconds: payload.RecommendedSyncDelaySeconds,
            TransportLossLevel: payload.TransportLossLevel,
            TransportAnomalyKind: payload.TransportAnomalyKind,
            TransportAnomalyReason: payload.TransportAnomalyReason,
            TransportAnomalyConfidence: payload.TransportAnomalyConfidence,
            ReceiverTelemetryAgeSeconds: payload.ReceiverTelemetryAgeSeconds,
            SenderTelemetryAgeSeconds: payload.SenderTelemetryAgeSeconds,
            LastRouteActionKind: payload.LastRouteActionKind,
            LastRouteActionReason: payload.LastRouteActionReason,
            LastRouteActionActor: payload.LastRouteActionActor,
            LastRouteActionUtc: payload.LastRouteActionUtc,
            RouteRecoveryCount: payload.RouteRecoveryCount,
            RouteRecoveryCooldownSeconds: payload.RouteRecoveryCooldownSeconds,
            RouteFallbackCount: payload.RouteFallbackCount,
            RouteFallbackCooldownSeconds: payload.RouteFallbackCooldownSeconds,
            StreamHost: payload.StreamEndpoint.Host,
            StreamPort: payload.StreamEndpoint.Port,
            RelayHost: payload.RelayEndpoint?.Host,
            RelayPort: payload.RelayEndpoint?.Port,
            RelayRegion: payload.RelayRegion,
            ProbeHost: payload.ProbeEndpoint?.Host,
            ProbePort: payload.ProbeEndpoint?.Port,
            ProbeToken: payload.ProbeToken,
            NatStatus: payload.NatStatus,
            HostNatProbeAgeSeconds: payload.HostNatProbeAgeSeconds,
            ClientNatProbeAgeSeconds: payload.ClientNatProbeAgeSeconds,
            NatProbeFresh: payload.NatProbeFresh,
            ReceiverHost: payload.ReceiverEndpoint?.Host,
            ReceiverPort: payload.ReceiverEndpoint?.Port);
    }

    public async Task<DesktopControlPlaneConnectInstructions> RecoverManagedSessionRouteAsync(
        string baseUrl,
        string sessionId,
        string sessionToken,
        string reason = "managed_route_recovery",
        CancellationToken cancellationToken = default)
    {
        var accessToken = await EnsureAccessTokenAsync(baseUrl, cancellationToken);
        using var request = CreateAuthorizedRequest(HttpMethod.Post, baseUrl, $"/api/sessions/{sessionId}/route/recover", accessToken);
        request.Content = JsonContent.Create(new SessionActionRequest(SessionToken: sessionToken, Reason: reason), options: _jsonOptions);

        using var response = await _httpClient.SendAsync(request, cancellationToken);
        await EnsureSuccessAsync(response, cancellationToken);
        var payload = await response.Content.ReadFromJsonAsync<SessionConnectInstructionsResponse>(_jsonOptions, cancellationToken)
            ?? throw new InvalidOperationException("Control plane returned empty recovery instructions.");

        return new DesktopControlPlaneConnectInstructions(
            SessionId: payload.SessionId,
            HostId: payload.HostId,
            HostDisplayName: payload.HostDisplayName,
            Status: payload.Status,
            RouteKind: payload.RouteKind,
            RouteState: payload.RouteState,
            RouteVersion: payload.RouteVersion,
            SessionHealth: payload.SessionHealth,
            SessionHealthReason: payload.SessionHealthReason,
            RouteActionHint: payload.RouteActionHint,
            RouteActionReason: payload.RouteActionReason,
            RouteFallbackReadyDurationSeconds: payload.RouteFallbackReadyDurationSeconds,
            RouteRecoveryReadyDurationSeconds: payload.RouteRecoveryReadyDurationSeconds,
            RecommendedSyncDelaySeconds: payload.RecommendedSyncDelaySeconds,
            TransportLossLevel: payload.TransportLossLevel,
            TransportAnomalyKind: payload.TransportAnomalyKind,
            TransportAnomalyReason: payload.TransportAnomalyReason,
            TransportAnomalyConfidence: payload.TransportAnomalyConfidence,
            ReceiverTelemetryAgeSeconds: payload.ReceiverTelemetryAgeSeconds,
            SenderTelemetryAgeSeconds: payload.SenderTelemetryAgeSeconds,
            LastRouteActionKind: payload.LastRouteActionKind,
            LastRouteActionReason: payload.LastRouteActionReason,
            LastRouteActionActor: payload.LastRouteActionActor,
            LastRouteActionUtc: payload.LastRouteActionUtc,
            RouteRecoveryCount: payload.RouteRecoveryCount,
            RouteRecoveryCooldownSeconds: payload.RouteRecoveryCooldownSeconds,
            RouteFallbackCount: payload.RouteFallbackCount,
            RouteFallbackCooldownSeconds: payload.RouteFallbackCooldownSeconds,
            StreamHost: payload.StreamEndpoint.Host,
            StreamPort: payload.StreamEndpoint.Port,
            RelayHost: payload.RelayEndpoint?.Host,
            RelayPort: payload.RelayEndpoint?.Port,
            RelayRegion: payload.RelayRegion,
            ProbeHost: payload.ProbeEndpoint?.Host,
            ProbePort: payload.ProbeEndpoint?.Port,
            ProbeToken: payload.ProbeToken,
            NatStatus: payload.NatStatus,
            HostNatProbeAgeSeconds: payload.HostNatProbeAgeSeconds,
            ClientNatProbeAgeSeconds: payload.ClientNatProbeAgeSeconds,
            NatProbeFresh: payload.NatProbeFresh,
            ReceiverHost: payload.ReceiverEndpoint?.Host,
            ReceiverPort: payload.ReceiverEndpoint?.Port);
    }

    public async Task ActivateSessionAsync(
        string baseUrl,
        string sessionId,
        string sessionToken,
        CancellationToken cancellationToken = default)
    {
        var accessToken = await EnsureAccessTokenAsync(baseUrl, cancellationToken);
        using var request = CreateAuthorizedRequest(HttpMethod.Post, baseUrl, $"/api/sessions/{sessionId}/activate", accessToken);
        request.Content = JsonContent.Create(new SessionActionRequest(SessionToken: sessionToken, Reason: "receiver_ready"), options: _jsonOptions);

        using var response = await _httpClient.SendAsync(request, cancellationToken);
        await EnsureSuccessAsync(response, cancellationToken);
    }

    public async Task StopSessionAsync(
        string baseUrl,
        string sessionId,
        string sessionToken,
        string reason,
        CancellationToken cancellationToken = default)
    {
        var accessToken = await EnsureAccessTokenAsync(baseUrl, cancellationToken);
        using var request = CreateAuthorizedRequest(HttpMethod.Post, baseUrl, $"/api/sessions/{sessionId}/stop", accessToken);
        request.Content = JsonContent.Create(new SessionActionRequest(SessionToken: sessionToken, Reason: reason), options: _jsonOptions);

        using var response = await _httpClient.SendAsync(request, cancellationToken);
        await EnsureSuccessAsync(response, cancellationToken);
    }

    public async Task StopSessionForActorAsync(
        string baseUrl,
        string sessionId,
        string reason,
        CancellationToken cancellationToken = default)
    {
        var accessToken = await EnsureAccessTokenAsync(baseUrl, cancellationToken);
        using var request = CreateAuthorizedRequest(HttpMethod.Post, baseUrl, $"/api/sessions/{sessionId}/stop", accessToken);
        request.Content = JsonContent.Create(new SessionActionRequest(SessionToken: string.Empty, Reason: reason), options: _jsonOptions);

        using var response = await _httpClient.SendAsync(request, cancellationToken);
        await EnsureSuccessAsync(response, cancellationToken);
    }

    public async Task<DesktopControlPlaneConnectInstructions> GetConnectInstructionsAsync(
        string baseUrl,
        string sessionId,
        string sessionToken,
        CancellationToken cancellationToken = default)
    {
        var accessToken = await EnsureAccessTokenAsync(baseUrl, cancellationToken);
        var path = $"/api/sessions/{sessionId}/connect?sessionToken={Uri.EscapeDataString(sessionToken)}";
        using var request = CreateAuthorizedRequest(HttpMethod.Get, baseUrl, path, accessToken);
        using var response = await _httpClient.SendAsync(request, cancellationToken);
        await EnsureSuccessAsync(response, cancellationToken);
        var payload = await response.Content.ReadFromJsonAsync<SessionConnectInstructionsResponse>(_jsonOptions, cancellationToken)
            ?? throw new InvalidOperationException("Control plane returned empty connect instructions.");

        return new DesktopControlPlaneConnectInstructions(
            SessionId: payload.SessionId,
            HostId: payload.HostId,
            HostDisplayName: payload.HostDisplayName,
            Status: payload.Status,
            RouteKind: payload.RouteKind,
            RouteState: payload.RouteState,
            RouteVersion: payload.RouteVersion,
            SessionHealth: payload.SessionHealth,
            SessionHealthReason: payload.SessionHealthReason,
            RouteActionHint: payload.RouteActionHint,
            RouteActionReason: payload.RouteActionReason,
            RouteFallbackReadyDurationSeconds: payload.RouteFallbackReadyDurationSeconds,
            RouteRecoveryReadyDurationSeconds: payload.RouteRecoveryReadyDurationSeconds,
            RecommendedSyncDelaySeconds: payload.RecommendedSyncDelaySeconds,
            TransportLossLevel: payload.TransportLossLevel,
            TransportAnomalyKind: payload.TransportAnomalyKind,
            TransportAnomalyReason: payload.TransportAnomalyReason,
            TransportAnomalyConfidence: payload.TransportAnomalyConfidence,
            ReceiverTelemetryAgeSeconds: payload.ReceiverTelemetryAgeSeconds,
            SenderTelemetryAgeSeconds: payload.SenderTelemetryAgeSeconds,
            LastRouteActionKind: payload.LastRouteActionKind,
            LastRouteActionReason: payload.LastRouteActionReason,
            LastRouteActionActor: payload.LastRouteActionActor,
            LastRouteActionUtc: payload.LastRouteActionUtc,
            RouteRecoveryCount: payload.RouteRecoveryCount,
            RouteRecoveryCooldownSeconds: payload.RouteRecoveryCooldownSeconds,
            RouteFallbackCount: payload.RouteFallbackCount,
            RouteFallbackCooldownSeconds: payload.RouteFallbackCooldownSeconds,
            StreamHost: payload.StreamEndpoint.Host,
            StreamPort: payload.StreamEndpoint.Port,
            RelayHost: payload.RelayEndpoint?.Host,
            RelayPort: payload.RelayEndpoint?.Port,
            RelayRegion: payload.RelayRegion,
            ProbeHost: payload.ProbeEndpoint?.Host,
            ProbePort: payload.ProbeEndpoint?.Port,
            ProbeToken: payload.ProbeToken,
            NatStatus: payload.NatStatus,
            HostNatProbeAgeSeconds: payload.HostNatProbeAgeSeconds,
            ClientNatProbeAgeSeconds: payload.ClientNatProbeAgeSeconds,
            NatProbeFresh: payload.NatProbeFresh,
            ReceiverHost: payload.ReceiverEndpoint?.Host,
            ReceiverPort: payload.ReceiverEndpoint?.Port);
    }

    public async Task<DesktopControlPlaneRoutePolicy> GetRoutePolicyAsync(
        string baseUrl,
        string sessionId,
        string sessionToken,
        CancellationToken cancellationToken = default)
    {
        var accessToken = await EnsureAccessTokenAsync(baseUrl, cancellationToken);
        var path = $"/api/sessions/{sessionId}/route/policy?sessionToken={Uri.EscapeDataString(sessionToken)}";
        using var request = CreateAuthorizedRequest(HttpMethod.Get, baseUrl, path, accessToken);
        using var response = await _httpClient.SendAsync(request, cancellationToken);
        await EnsureSuccessAsync(response, cancellationToken);
        var payload = await response.Content.ReadFromJsonAsync<SessionRoutePolicyResponse>(_jsonOptions, cancellationToken)
            ?? throw new InvalidOperationException("Control plane returned empty route policy.");

        return new DesktopControlPlaneRoutePolicy(
            SessionId: payload.SessionId,
            HostId: payload.HostId,
            RouteKind: payload.RouteKind,
            RouteState: payload.RouteState,
            RouteVersion: payload.RouteVersion,
            SessionHealth: payload.SessionHealth,
            SessionHealthReason: payload.SessionHealthReason,
            RouteActionHint: payload.RouteActionHint,
            RouteActionReason: payload.RouteActionReason,
            RecommendedSyncDelaySeconds: payload.RecommendedSyncDelaySeconds,
            TransportLossLevel: payload.TransportLossLevel,
            TransportAnomalyKind: payload.TransportAnomalyKind,
            TransportAnomalyReason: payload.TransportAnomalyReason,
            TransportAnomalyConfidence: payload.TransportAnomalyConfidence,
            ActionableAnomaly: payload.ActionableAnomaly,
            HighConfidenceAnomaly: payload.HighConfidenceAnomaly,
            FallbackWarmupSeconds: payload.FallbackWarmupSeconds,
            FallbackReadyDurationSeconds: payload.FallbackReadyDurationSeconds,
            FallbackReady: payload.FallbackReady,
            RecoveryWarmupSeconds: payload.RecoveryWarmupSeconds,
            RecoveryReadyDurationSeconds: payload.RecoveryReadyDurationSeconds,
            RecoveryReady: payload.RecoveryReady,
            FallbackCooldownSeconds: payload.FallbackCooldownSeconds,
            RecoveryCooldownSeconds: payload.RecoveryCooldownSeconds,
            ReceiverTelemetryAgeSeconds: payload.ReceiverTelemetryAgeSeconds,
            SenderTelemetryAgeSeconds: payload.SenderTelemetryAgeSeconds,
            NatStatus: payload.NatStatus,
            HostNatProbeAgeSeconds: payload.HostNatProbeAgeSeconds,
            ClientNatProbeAgeSeconds: payload.ClientNatProbeAgeSeconds,
            NatProbeFresh: payload.NatProbeFresh);
    }

    public async Task PublishNatProbeAsync(
        string baseUrl,
        string sessionId,
        string sessionToken,
        string probeToken,
        string probeHost,
        int probePort,
        string role,
        CancellationToken cancellationToken = default)
    {
        if (string.IsNullOrWhiteSpace(probeToken) ||
            string.IsNullOrWhiteSpace(probeHost) ||
            probePort is < 1 or > 65535)
        {
            return;
        }

        var observed = await RunNatProbeAsync(sessionId, probeToken, probeHost, probePort, role, cancellationToken);
        if (observed is null)
        {
            return;
        }

        var accessToken = await EnsureAccessTokenAsync(baseUrl, cancellationToken);
        using var request = CreateAuthorizedRequest(HttpMethod.Post, baseUrl, $"/api/sessions/{sessionId}/nat/probe", accessToken);
        request.Content = JsonContent.Create(
            new SessionNatProbeRequest(
                SessionToken: sessionToken,
                ProbeToken: probeToken,
                Role: role,
                ObservedAddress: observed.ObservedAddress,
                ObservedPort: observed.ObservedPort,
                LocalAddress: observed.LocalAddress,
                LocalPort: observed.LocalPort,
                NetworkType: "udp"),
            options: _jsonOptions);

        using var response = await _httpClient.SendAsync(request, cancellationToken);
        await EnsureSuccessAsync(response, cancellationToken);
    }

    private async Task<string> EnsureAccessTokenAsync(string baseUrl, CancellationToken cancellationToken)
    {
        var normalizedBaseUrl = NormalizeBaseUrl(baseUrl);
        if (_accessSession is not null &&
            string.Equals(_accessSession.BaseUrl, normalizedBaseUrl, StringComparison.OrdinalIgnoreCase) &&
            _accessSession.ExpiresUtc > DateTimeOffset.UtcNow.AddMinutes(1))
        {
            return _accessSession.AccessToken;
        }

        _persistedState ??= LoadPersistedState();
        if (_persistedState is not null &&
            string.Equals(_persistedState.BaseUrl, normalizedBaseUrl, StringComparison.OrdinalIgnoreCase) &&
            !string.IsNullOrWhiteSpace(_persistedState.UserRefreshToken) &&
            _persistedState.UserRefreshExpiresUtc > DateTimeOffset.UtcNow.AddMinutes(1))
        {
            try
            {
                using var refreshRequest = new HttpRequestMessage(HttpMethod.Post, BuildUri(normalizedBaseUrl, "/api/auth/users/refresh"))
                {
                    Content = JsonContent.Create(
                        new UserRefreshAccessTokenRequest(_persistedState.UserRefreshToken),
                        options: _jsonOptions),
                };
                using var refreshResponse = await _httpClient.SendAsync(refreshRequest, cancellationToken);
                await EnsureSuccessAsync(refreshResponse, cancellationToken);
                var refreshPayload = await refreshResponse.Content.ReadFromJsonAsync<UserLoginResponse>(_jsonOptions, cancellationToken)
                    ?? throw new InvalidOperationException("Control plane user refresh returned an empty response.");

                _persistedState = _persistedState with
                {
                    UserEmail = refreshPayload.User.Email,
                    UserRefreshToken = refreshPayload.RefreshToken,
                    UserRefreshExpiresUtc = refreshPayload.RefreshExpiresUtc,
                };
                SavePersistedState(_persistedState);
                _accessSession = new DesktopControlPlaneAccessSession(
                    BaseUrl: normalizedBaseUrl,
                    AccessToken: refreshPayload.AccessToken,
                    ExpiresUtc: refreshPayload.ExpiresUtc);
                return _accessSession.AccessToken;
            }
            catch
            {
            }
        }

        if (_persistedState is not null &&
            string.Equals(_persistedState.BaseUrl, normalizedBaseUrl, StringComparison.OrdinalIgnoreCase) &&
            !string.IsNullOrWhiteSpace(_persistedState.RefreshToken) &&
            _persistedState.RefreshExpiresUtc > DateTimeOffset.UtcNow.AddMinutes(1))
        {
            try
            {
                using var refreshRequest = new HttpRequestMessage(HttpMethod.Post, BuildUri(normalizedBaseUrl, "/api/auth/refresh"))
                {
                    Content = JsonContent.Create(
                        new RefreshAccessTokenRequest(_persistedState.RefreshToken),
                        options: _jsonOptions),
                };
                using var refreshResponse = await _httpClient.SendAsync(refreshRequest, cancellationToken);
                await EnsureSuccessAsync(refreshResponse, cancellationToken);
                var refreshPayload = await refreshResponse.Content.ReadFromJsonAsync<RefreshAccessTokenResponse>(_jsonOptions, cancellationToken)
                    ?? throw new InvalidOperationException("Control plane refresh returned an empty response.");

                _persistedState = _persistedState with
                {
                    RefreshToken = refreshPayload.RefreshToken,
                    RefreshExpiresUtc = refreshPayload.RefreshExpiresUtc,
                };
                SavePersistedState(_persistedState);
                _accessSession = new DesktopControlPlaneAccessSession(
                    BaseUrl: normalizedBaseUrl,
                    AccessToken: refreshPayload.AccessToken,
                    ExpiresUtc: refreshPayload.ExpiresUtc);
                return _accessSession.AccessToken;
            }
            catch
            {
            }
        }

        var requestBody = new DeviceLoginRequest(
            DeviceId: string.Equals(_persistedState?.BaseUrl, normalizedBaseUrl, StringComparison.OrdinalIgnoreCase) ? _persistedState?.DeviceId : null,
            DeviceSecret: string.Equals(_persistedState?.BaseUrl, normalizedBaseUrl, StringComparison.OrdinalIgnoreCase) ? _persistedState?.DeviceSecret : null,
            DeviceLabel: $"{Environment.MachineName} desktop",
            Platform: "windows");

        using var request = new HttpRequestMessage(HttpMethod.Post, BuildUri(normalizedBaseUrl, "/api/auth/device-login"))
        {
            Content = JsonContent.Create(requestBody, options: _jsonOptions),
        };
        using var response = await _httpClient.SendAsync(request, cancellationToken);
        await EnsureSuccessAsync(response, cancellationToken);
        var payload = await response.Content.ReadFromJsonAsync<DeviceLoginResponse>(_jsonOptions, cancellationToken)
            ?? throw new InvalidOperationException("Control plane auth returned an empty response.");

        _persistedState = new DesktopControlPlanePersistedState(
            BaseUrl: normalizedBaseUrl,
            DeviceId: payload.DeviceId,
            DeviceSecret: payload.DeviceSecret,
            RefreshToken: payload.RefreshToken,
            RefreshExpiresUtc: payload.RefreshExpiresUtc,
            UserEmail: _persistedState?.UserEmail,
            UserRefreshToken: _persistedState?.UserRefreshToken,
            UserRefreshExpiresUtc: _persistedState?.UserRefreshExpiresUtc,
            ManagedSession: _persistedState?.ManagedSession);
        SavePersistedState(_persistedState);

        _accessSession = new DesktopControlPlaneAccessSession(
            BaseUrl: normalizedBaseUrl,
            AccessToken: payload.AccessToken,
            ExpiresUtc: payload.ExpiresUtc);
        return _accessSession.AccessToken;
    }

    private async Task<DesktopControlPlaneAuthState> AuthenticateUserAsync(string baseUrl, string path, string email, string password, CancellationToken cancellationToken)
    {
        var normalizedBaseUrl = NormalizeBaseUrl(baseUrl);
        if (string.IsNullOrWhiteSpace(normalizedBaseUrl))
        {
            throw new InvalidOperationException("Control plane URL is required.");
        }

        using var request = new HttpRequestMessage(HttpMethod.Post, BuildUri(normalizedBaseUrl, path))
        {
            Content = JsonContent.Create(new UserLoginRequest(email.Trim(), password), options: _jsonOptions),
        };
        using var response = await _httpClient.SendAsync(request, cancellationToken);
        await EnsureSuccessAsync(response, cancellationToken);
        var payload = await response.Content.ReadFromJsonAsync<UserLoginResponse>(_jsonOptions, cancellationToken)
            ?? throw new InvalidOperationException("User auth returned an empty response.");

        _persistedState ??= LoadPersistedState();
        _persistedState = (_persistedState ?? new DesktopControlPlanePersistedState(
            BaseUrl: normalizedBaseUrl,
            DeviceId: string.Empty,
            DeviceSecret: string.Empty,
            RefreshToken: string.Empty,
            RefreshExpiresUtc: DateTimeOffset.MinValue,
            UserEmail: null,
            UserRefreshToken: null,
            UserRefreshExpiresUtc: null,
            ManagedSession: null)) with
        {
            BaseUrl = normalizedBaseUrl,
            UserEmail = payload.User.Email,
            UserRefreshToken = payload.RefreshToken,
            UserRefreshExpiresUtc = payload.RefreshExpiresUtc,
        };
        SavePersistedState(_persistedState);

        _accessSession = new DesktopControlPlaneAccessSession(
            BaseUrl: normalizedBaseUrl,
            AccessToken: payload.AccessToken,
            ExpiresUtc: payload.ExpiresUtc);
        return new DesktopControlPlaneAuthState("user", payload.User.Email, UserAuthenticated: true);
    }

    private async Task<NatProbeEcho?> RunNatProbeAsync(string sessionId, string probeToken, string probeHost, int probePort, string role, CancellationToken cancellationToken)
    {
        using var udpClient = new UdpClient(AddressFamily.InterNetwork);
        udpClient.Client.ReceiveTimeout = 2000;
        var request = JsonSerializer.SerializeToUtf8Bytes(new NatProbeWireRequest(
            Kind: "nat_probe",
            SessionId: sessionId,
            ProbeToken: probeToken,
            Role: role), _jsonOptions);

        await udpClient.SendAsync(request, request.Length, probeHost, probePort);
        UdpReceiveResult response;
        try
        {
            response = await udpClient.ReceiveAsync(cancellationToken);
        }
        catch
        {
            return null;
        }

        var payload = JsonSerializer.Deserialize<NatProbeWireResponse>(response.Buffer, _jsonOptions);
        if (payload is null ||
            !string.Equals(payload.Kind, "nat_probe_ack", StringComparison.OrdinalIgnoreCase) ||
            !string.Equals(payload.SessionId, sessionId, StringComparison.Ordinal) ||
            !string.Equals(payload.ProbeToken, probeToken, StringComparison.Ordinal) ||
            string.IsNullOrWhiteSpace(payload.ObservedAddress) ||
            payload.ObservedPort is < 1 or > 65535)
        {
            return null;
        }

        var local = udpClient.Client.LocalEndPoint as IPEndPoint;
        return new NatProbeEcho(
            ObservedAddress: payload.ObservedAddress.Trim(),
            ObservedPort: payload.ObservedPort,
            LocalAddress: local?.Address.ToString(),
            LocalPort: local?.Port);
    }

    private static HttpRequestMessage CreateAuthorizedRequest(HttpMethod method, string baseUrl, string path, string accessToken)
    {
        var request = new HttpRequestMessage(method, BuildUri(baseUrl, path));
        request.Headers.Authorization = new AuthenticationHeaderValue("Bearer", accessToken);
        return request;
    }

    private static Uri BuildUri(string baseUrl, string path) =>
        new($"{NormalizeBaseUrl(baseUrl)}{path}", UriKind.Absolute);

    private static string NormalizeBaseUrl(string? baseUrl) =>
        string.IsNullOrWhiteSpace(baseUrl) ? string.Empty : baseUrl.Trim().TrimEnd('/');

    private static async Task EnsureSuccessAsync(HttpResponseMessage response, CancellationToken cancellationToken)
    {
        if (response.IsSuccessStatusCode)
        {
            return;
        }

        var body = await response.Content.ReadAsStringAsync(cancellationToken);
        if (!string.IsNullOrWhiteSpace(body))
        {
            try
            {
                var apiError = JsonSerializer.Deserialize<ApiErrorResponse>(body, new JsonSerializerOptions(JsonSerializerDefaults.Web));
                if (!string.IsNullOrWhiteSpace(apiError?.Message))
                {
                    throw new ControlPlaneApiException(apiError.Code ?? string.Empty, apiError.Message);
                }
            }
            catch (JsonException)
            {
            }
        }

        throw new InvalidOperationException($"Control plane request failed: {(int)response.StatusCode} {response.ReasonPhrase}");
    }

    private DesktopControlPlanePersistedState? LoadPersistedState()
    {
        try
        {
            if (!File.Exists(_statePath))
            {
                return null;
            }

            return JsonSerializer.Deserialize<DesktopControlPlanePersistedState>(
                File.ReadAllText(_statePath),
                _jsonOptions);
        }
        catch
        {
            return null;
        }
    }

    private void SavePersistedState(DesktopControlPlanePersistedState state)
    {
        try
        {
            File.WriteAllText(_statePath, JsonSerializer.Serialize(state, _jsonOptions));
        }
        catch
        {
        }
    }

    private sealed record DesktopControlPlanePersistedState(
        string BaseUrl,
        string DeviceId,
        string DeviceSecret,
        string RefreshToken,
        DateTimeOffset RefreshExpiresUtc,
        string? UserEmail,
        string? UserRefreshToken,
        DateTimeOffset? UserRefreshExpiresUtc,
        DesktopControlPlaneManagedSessionState? ManagedSession);

    private sealed record DesktopControlPlaneAccessSession(string BaseUrl, string AccessToken, DateTimeOffset ExpiresUtc);

    private sealed record DeviceLoginRequest(string? DeviceId, string? DeviceSecret, string DeviceLabel, string Platform);

    private sealed record DeviceLoginResponse(
        string DeviceId,
        string DeviceSecret,
        string AccessToken,
        DateTimeOffset ExpiresUtc,
        string RefreshToken,
        DateTimeOffset RefreshExpiresUtc);

    private sealed record RefreshAccessTokenRequest(string RefreshToken);

    private sealed record RefreshAccessTokenResponse(
        string AccessToken,
        DateTimeOffset ExpiresUtc,
        string RefreshToken,
        DateTimeOffset RefreshExpiresUtc);

    private sealed record UserLoginRequest(string Email, string Password);

    private sealed record UserRefreshAccessTokenRequest(string RefreshToken);

    private sealed record UserSummary(string UserId, string Email);

    private sealed record UserLoginResponse(
        string AccessToken,
        DateTimeOffset ExpiresUtc,
        string RefreshToken,
        DateTimeOffset RefreshExpiresUtc,
        UserSummary User);

    private sealed record CreateSessionRequest(
        string HostId,
        string ClientLabel,
        string ClientRegion,
        string? CodecPreference,
        string[]? PreferredCodecs,
        string? PresetId,
        bool PreferRelay,
        bool ReplaceExistingActorSession,
        bool AudioRequested,
        int ControllerCount,
        int LeaseMinutes,
        string ReceiverAddress,
        int ReceiverPort,
        int RequestedWidth,
        int RequestedHeight,
        int RequestedFps,
        int RequestedBitrateBps,
        bool? CaptureCursor,
        bool? AdaptiveMode,
        ClientCapabilitiesRequest? Capabilities);

    private sealed record ClientCapabilitiesRequest(
        string[]? SupportedDecodeCodecs,
        string[]? LanAddresses);

    private sealed record SessionActionRequest(string SessionToken, string Reason);

    private sealed record SessionLeaseResponse(
        string SessionId,
        string SessionToken,
        string HostId,
        [property: JsonConverter(typeof(SessionStatusJsonConverter))] string Status,
        string RouteKind,
        string RouteState,
        int RouteVersion,
        string SessionHealth,
        string SessionHealthReason,
        string RouteActionHint,
        string RouteActionReason,
        int RouteFallbackReadyDurationSeconds,
        int RouteRecoveryReadyDurationSeconds,
        int RecommendedSyncDelaySeconds,
        string TransportLossLevel,
        string TransportAnomalyKind,
        string TransportAnomalyReason,
        string TransportAnomalyConfidence,
        int ReceiverTelemetryAgeSeconds,
        int SenderTelemetryAgeSeconds,
        string? LastRouteActionKind,
        string? LastRouteActionReason,
        string? LastRouteActionActor,
        DateTimeOffset? LastRouteActionUtc,
        int RouteRecoveryCount,
        int RouteRecoveryCooldownSeconds,
        int RouteFallbackCount,
        int RouteFallbackCooldownSeconds,
        StreamEndpoint? ReceiverEndpoint,
        StreamEndpoint? RelayEndpoint,
        string? RelayRegion,
        StreamEndpoint? ProbeEndpoint,
        string ProbeToken,
        string NatStatus,
        int HostNatProbeAgeSeconds,
        int ClientNatProbeAgeSeconds,
        bool NatProbeFresh,
        string? CodecPreference,
        string HostDisplayName);

    private sealed record SessionConnectInstructionsResponse(
        string SessionId,
        string HostId,
        string HostDisplayName,
        [property: JsonConverter(typeof(SessionStatusJsonConverter))] string Status,
        string RouteKind,
        string RouteState,
        int RouteVersion,
        string SessionHealth,
        string SessionHealthReason,
        string RouteActionHint,
        string RouteActionReason,
        int RouteFallbackReadyDurationSeconds,
        int RouteRecoveryReadyDurationSeconds,
        int RecommendedSyncDelaySeconds,
        string TransportLossLevel,
        string TransportAnomalyKind,
        string TransportAnomalyReason,
        string TransportAnomalyConfidence,
        int ReceiverTelemetryAgeSeconds,
        int SenderTelemetryAgeSeconds,
        string? LastRouteActionKind,
        string? LastRouteActionReason,
        string? LastRouteActionActor,
        DateTimeOffset? LastRouteActionUtc,
        int RouteRecoveryCount,
        int RouteRecoveryCooldownSeconds,
        int RouteFallbackCount,
        int RouteFallbackCooldownSeconds,
        StreamEndpoint StreamEndpoint,
        StreamEndpoint? ReceiverEndpoint,
        StreamEndpoint? RelayEndpoint,
        string? RelayRegion,
        StreamEndpoint? ProbeEndpoint,
        string ProbeToken,
        string NatStatus,
        int HostNatProbeAgeSeconds,
        int ClientNatProbeAgeSeconds,
        bool NatProbeFresh);

    private sealed record SessionRoutePolicyResponse(
        string SessionId,
        string HostId,
        string RouteKind,
        string RouteState,
        int RouteVersion,
        string SessionHealth,
        string SessionHealthReason,
        string RouteActionHint,
        string RouteActionReason,
        int RecommendedSyncDelaySeconds,
        string TransportLossLevel,
        string TransportAnomalyKind,
        string TransportAnomalyReason,
        string TransportAnomalyConfidence,
        bool ActionableAnomaly,
        bool HighConfidenceAnomaly,
        int FallbackWarmupSeconds,
        int FallbackReadyDurationSeconds,
        bool FallbackReady,
        int RecoveryWarmupSeconds,
        int RecoveryReadyDurationSeconds,
        bool RecoveryReady,
        int FallbackCooldownSeconds,
        int RecoveryCooldownSeconds,
        int ReceiverTelemetryAgeSeconds,
        int SenderTelemetryAgeSeconds,
        string NatStatus,
        int HostNatProbeAgeSeconds,
        int ClientNatProbeAgeSeconds,
        bool NatProbeFresh);

    private sealed record SessionNatProbeRequest(
        string SessionToken,
        string ProbeToken,
        string Role,
        string ObservedAddress,
        int ObservedPort,
        string? LocalAddress,
        int? LocalPort,
        string? NetworkType);

    private sealed record StreamEndpoint(string Host, int Port, string Transport);

    private sealed record NatProbeWireRequest(string Kind, string SessionId, string ProbeToken, string Role);

    private sealed record NatProbeWireResponse(string Kind, string SessionId, string ProbeToken, string ObservedAddress, int ObservedPort);

    private sealed record NatProbeEcho(string ObservedAddress, int ObservedPort, string? LocalAddress, int? LocalPort);

    private sealed record ApiErrorResponse(string Code, string Message);
}
