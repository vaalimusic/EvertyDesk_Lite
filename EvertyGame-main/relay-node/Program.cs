using System.Buffers.Binary;
using System.Net;
using System.Net.Http.Json;
using System.Net.Sockets;
using System.Security.Cryptography;
using System.Text.Json;

var settings = RelayNodeSettings.FromArgs(args);
using var runtime = new RelayNodeRuntime(settings);
using var cts = new CancellationTokenSource();

Console.CancelKeyPress += (_, eventArgs) =>
{
    eventArgs.Cancel = true;
    cts.Cancel();
};

await runtime.RunAsync(cts.Token);

sealed class RelayNodeRuntime : IDisposable
{
    private static readonly TimeSpan RegistrationTtl = TimeSpan.FromSeconds(12);

    private readonly RelayNodeSettings _settings;
    private readonly HttpClient _httpClient = new() { Timeout = TimeSpan.FromSeconds(5) };
    private readonly JsonSerializerOptions _jsonOptions = new(JsonSerializerDefaults.Web);
    private readonly object _sync = new();
    private readonly Dictionary<string, RelaySessionRuntime> _sessions = new(StringComparer.Ordinal);
    private readonly string _statePath;

    private UdpClient? _udpClient;
    private RelayRegistrationState? _registrationState;

    public RelayNodeRuntime(RelayNodeSettings settings)
    {
        _settings = settings;
        var directory = Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
            "EvertyRelayNode");
        Directory.CreateDirectory(directory);
        _statePath = Path.Combine(directory, "relay-node.json");
    }

    public void Dispose()
    {
        _udpClient?.Dispose();
        _httpClient.Dispose();
    }

    public async Task RunAsync(CancellationToken cancellationToken)
    {
        _registrationState = LoadPersistedState();
        await EnsureRegisteredAsync(cancellationToken);

        _udpClient = new UdpClient(_settings.UdpPort);
        _udpClient.Client.ReceiveBufferSize = 2 * 1024 * 1024;
        _udpClient.Client.SendBufferSize = 2 * 1024 * 1024;

        Console.WriteLine($"Relay node listening on UDP {_settings.UdpPort}, public {_settings.PublicAddress}:{_settings.UdpPort}");

        var heartbeatTask = Task.Run(() => HeartbeatLoopAsync(cancellationToken), cancellationToken);
        try
        {
            await ReceiveLoopAsync(cancellationToken);
        }
        finally
        {
            try
            {
                await heartbeatTask;
            }
            catch
            {
            }
        }
    }

    private async Task ReceiveLoopAsync(CancellationToken cancellationToken)
    {
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

            var now = DateTimeOffset.UtcNow;
            CleanupExpiredSessions(now);

            if (RelayProtocol.TryParseNatProbe(result.Buffer, result.Buffer.Length, out var natProbe))
            {
                Console.WriteLine(
                    $"Relay nat_probe {result.RemoteEndPoint.Address}:{result.RemoteEndPoint.Port} " +
                    $"session={natProbe!.SessionId} role={natProbe.Role} bytes={result.Buffer.Length}");
                await SendNatProbeAckAsync(result.RemoteEndPoint, natProbe!, cancellationToken);
                continue;
            }

            if (RelayProtocol.TryParseRelayRegistration(result.Buffer, result.Buffer.Length, out var registration))
            {
                Console.WriteLine(
                    $"Relay relay_register {result.RemoteEndPoint.Address}:{result.RemoteEndPoint.Port} " +
                    $"session={registration!.SessionId} role={registration.Role} bytes={result.Buffer.Length}");
                HandleRegistration(registration!, result.RemoteEndPoint, now);
                continue;
            }

            var controlDescription = RelayProtocol.TryDescribeControlPacket(result.Buffer, result.Buffer.Length, out var description)
                ? description
                : description;
            Console.WriteLine(
                $"Relay ignored datagram from {result.RemoteEndPoint.Address}:{result.RemoteEndPoint.Port} " +
                $"bytes={result.Buffer.Length} desc={controlDescription}");

            if (!TryResolveForwardTarget(result.RemoteEndPoint, now, out var target))
            {
                continue;
            }

            try
            {
                Console.WriteLine($"Relay forward {result.RemoteEndPoint.Address}:{result.RemoteEndPoint.Port} -> {target.Address}:{target.Port} ({result.Buffer.Length} bytes)");
                await _udpClient!.SendAsync(result.Buffer, result.Buffer.Length, target);
            }
            catch
            {
            }
        }
    }

    private async Task SendNatProbeAckAsync(IPEndPoint remoteEndPoint, NatProbeRequest natProbe, CancellationToken cancellationToken)
    {
        var payload = JsonSerializer.SerializeToUtf8Bytes(new NatProbeResponse(
            Kind: "nat_probe_ack",
            SessionId: natProbe.SessionId,
            ProbeToken: natProbe.ProbeToken,
            ObservedAddress: remoteEndPoint.Address.ToString(),
            ObservedPort: remoteEndPoint.Port), _jsonOptions);

        try
        {
            await _udpClient!.SendAsync(payload, payload.Length, remoteEndPoint);
        }
        catch
        {
        }
    }

    private async Task SendRelayRegistrationAckAsync(RelayRegistrationMessage registration, IPEndPoint remoteEndPoint)
    {
        var payload = JsonSerializer.SerializeToUtf8Bytes(new RelayRegistrationAck(
            Kind: "relay_register_ack",
            SessionId: registration.SessionId,
            Role: registration.Role,
            ObservedAddress: remoteEndPoint.Address.ToString(),
            ObservedPort: remoteEndPoint.Port), _jsonOptions);
        var packet = RelayProtocol.BuildControlPacket(payload);

        try
        {
            await _udpClient!.SendAsync(packet, packet.Length, remoteEndPoint);
            Console.WriteLine(
                $"Relay relay_register_ack {remoteEndPoint.Address}:{remoteEndPoint.Port} " +
                $"session={registration.SessionId} role={registration.Role}");
        }
        catch (Exception ex)
        {
            Console.WriteLine(
                $"Relay relay_register_ack failed {remoteEndPoint.Address}:{remoteEndPoint.Port} " +
                $"session={registration.SessionId}: {ex.Message}");
        }
    }

    private void HandleRegistration(RelayRegistrationMessage registration, IPEndPoint remoteEndPoint, DateTimeOffset now)
    {
        lock (_sync)
        {
            var staleKeys = _sessions
                .Where(pair =>
                    pair.Key != registration.SessionId &&
                    registration.Role switch
                    {
                        "receiver" => pair.Value.RegisteredReceiverEndpoint is not null &&
                                      EndpointsEqual(pair.Value.RegisteredReceiverEndpoint, remoteEndPoint),
                        "sender" => pair.Value.RegisteredSenderEndpoint is not null &&
                                    EndpointsEqual(pair.Value.RegisteredSenderEndpoint, remoteEndPoint),
                        _ => false
                    })
                .Select(pair => pair.Key)
                .ToList();

            foreach (var staleKey in staleKeys)
            {
                _sessions.Remove(staleKey);
                Console.WriteLine(
                    $"Relay: evicting stale {registration.Role} session {staleKey} -> replaced by {registration.SessionId}.");
            }

            PurgeConflictingEndpointMappings(registration.SessionId, registration.Role, remoteEndPoint);

            if (!_sessions.TryGetValue(registration.SessionId, out var session))
            {
                session = new RelaySessionRuntime(
                    registration.SessionId,
                    registration.SessionToken,
                    RegisteredSenderEndpoint: null,
                    RegisteredSenderLastSeenUtc: DateTimeOffset.MinValue,
                    RegisteredReceiverEndpoint: null,
                    RegisteredReceiverLastSeenUtc: DateTimeOffset.MinValue,
                    ControlPlaneSenderEndpoint: null,
                    ControlPlaneReceiverEndpoint: null);
            }
            else if (!FixedTimeEquals(session.SessionToken, registration.SessionToken))
            {
                return;
            }

            session = registration.Role switch
            {
                "sender" => session with
                {
                    RegisteredSenderEndpoint = remoteEndPoint,
                    RegisteredSenderLastSeenUtc = now,
                },
                "receiver" => session with
                {
                    RegisteredReceiverEndpoint = remoteEndPoint,
                    RegisteredReceiverLastSeenUtc = now,
                },
                _ => session,
            };

            _sessions[registration.SessionId] = session;
            Console.WriteLine(
                $"Relay registered {registration.Role} for session {registration.SessionId}: " +
                $"{remoteEndPoint.Address}:{remoteEndPoint.Port}; " +
                $"registered_sender={session.RegisteredSenderEndpoint?.Address}:{session.RegisteredSenderEndpoint?.Port}; " +
                $"registered_receiver={session.RegisteredReceiverEndpoint?.Address}:{session.RegisteredReceiverEndpoint?.Port}");
        }

        _ = SendRelayRegistrationAckAsync(registration, remoteEndPoint);

        _ = Task.Run(async () =>
        {
            try
            {
                await PublishRelayRegistrationAsync(registration.SessionId, registration.SessionToken, registration.Role, remoteEndPoint);
                await SyncSessionFromControlPlaneAsync(registration.SessionId, registration.SessionToken, registration.Role, remoteEndPoint, now);
            }
            catch (Exception ex)
            {
                Console.WriteLine($"Relay session sync failed for {registration.SessionId}: {ex.Message}");
            }
        });
    }

    private async Task PublishRelayRegistrationAsync(string sessionId, string sessionToken, string role, IPEndPoint remoteEndPoint)
    {
        var request = new RelaySessionRegistrationRequest(
            SessionToken: sessionToken,
            Role: role,
            ObservedAddress: remoteEndPoint.Address.ToString(),
            ObservedPort: remoteEndPoint.Port);

        using var response = await _httpClient.PostAsJsonAsync(
            BuildUri(_settings.ControlPlaneBaseUrl, $"/api/sessions/{sessionId}/relay/register"),
            request,
            _jsonOptions);
        response.EnsureSuccessStatusCode();
        Console.WriteLine(
            $"Relay published registration session={sessionId} role={role} observed={remoteEndPoint.Address}:{remoteEndPoint.Port}");
    }

    private async Task SyncSessionFromControlPlaneAsync(string sessionId, string sessionToken, string role, IPEndPoint remoteEndPoint, DateTimeOffset now)
    {
        using var httpResponse = await _httpClient.GetAsync(
            BuildUri(_settings.ControlPlaneBaseUrl, $"/api/sessions/{sessionId}/connect?sessionToken={Uri.EscapeDataString(sessionToken)}"));

        if (httpResponse.StatusCode is HttpStatusCode.NotFound)
        {
            lock (_sync)
            {
                if (_sessions.Remove(sessionId))
                {
                    Console.WriteLine($"Relay removed deleted session mapping {sessionId}.");
                }
            }
            return;
        }

        if (!httpResponse.IsSuccessStatusCode)
        {
            return;
        }

        var response = await httpResponse.Content.ReadFromJsonAsync<SessionConnectSnapshot>(_jsonOptions);
        if (response is null)
        {
            return;
        }

        if (IsTerminalSessionStatus(response.Status))
        {
            lock (_sync)
            {
                if (_sessions.TryGetValue(sessionId, out var dyingSession))
                {
                    _sessions.Remove(sessionId);

                    var conflictKeys = _sessions
                        .Where(pair =>
                            (dyingSession.RegisteredReceiverEndpoint is not null &&
                             pair.Value.RegisteredReceiverEndpoint is not null &&
                             EndpointsEqual(pair.Value.RegisteredReceiverEndpoint, dyingSession.RegisteredReceiverEndpoint)) ||
                            (dyingSession.RegisteredSenderEndpoint is not null &&
                             pair.Value.RegisteredSenderEndpoint is not null &&
                             EndpointsEqual(pair.Value.RegisteredSenderEndpoint, dyingSession.RegisteredSenderEndpoint)))
                        .Select(pair => pair.Key)
                        .ToList();

                    foreach (var key in conflictKeys)
                    {
                        _sessions.Remove(key);
                        Console.WriteLine($"Relay: evicting endpoint-conflicting session {key} on terminal cleanup of {sessionId}.");
                    }
                }
            }
            Console.WriteLine($"Relay removed terminal session mapping {sessionId}.");
            return;
        }

        var senderEndpoint = response.StreamEndpoint is not null
            ? new IPEndPoint(IPAddress.Parse(response.StreamEndpoint.Host), response.StreamEndpoint.Port)
            : null;
        var receiverEndpoint = response.ReceiverEndpoint is not null
            ? new IPEndPoint(IPAddress.Parse(response.ReceiverEndpoint.Host), response.ReceiverEndpoint.Port)
            : null;

        lock (_sync)
        {
            if (!_sessions.TryGetValue(sessionId, out var session))
            {
                session = new RelaySessionRuntime(
                    sessionId,
                    sessionToken,
                    RegisteredSenderEndpoint: null,
                    RegisteredSenderLastSeenUtc: DateTimeOffset.MinValue,
                    RegisteredReceiverEndpoint: null,
                    RegisteredReceiverLastSeenUtc: DateTimeOffset.MinValue,
                    ControlPlaneSenderEndpoint: null,
                    ControlPlaneReceiverEndpoint: null);
            }

            if (!FixedTimeEquals(session.SessionToken, sessionToken))
            {
                return;
            }

            session = session with
            {
                ControlPlaneSenderEndpoint = senderEndpoint,
                ControlPlaneReceiverEndpoint = receiverEndpoint,
            };

            _sessions[sessionId] = session;
            Console.WriteLine(
                $"Relay session sync {sessionId}: registered_sender={session.RegisteredSenderEndpoint?.Address}:{session.RegisteredSenderEndpoint?.Port}; " +
                $"registered_receiver={session.RegisteredReceiverEndpoint?.Address}:{session.RegisteredReceiverEndpoint?.Port}; " +
                $"contract_sender={session.ControlPlaneSenderEndpoint?.Address}:{session.ControlPlaneSenderEndpoint?.Port}; " +
                $"contract_receiver={session.ControlPlaneReceiverEndpoint?.Address}:{session.ControlPlaneReceiverEndpoint?.Port}; " +
                $"role={role}; source={remoteEndPoint.Address}:{remoteEndPoint.Port}");
        }
    }

    private static bool IsTerminalSessionStatus(JsonElement? status)
    {
        if (status is null)
        {
            return false;
        }

        var value = status.Value;
        return value.ValueKind switch
        {
            JsonValueKind.String => string.Equals(value.GetString(), "Stopped", StringComparison.OrdinalIgnoreCase) ||
                                    string.Equals(value.GetString(), "Expired", StringComparison.OrdinalIgnoreCase),
            JsonValueKind.Number => value.TryGetInt32(out var numericStatus) && (numericStatus == 2 || numericStatus == 3),
            _ => false,
        };
    }

    private void PurgeConflictingEndpointMappings(string sessionId, string role, IPEndPoint remoteEndPoint)
    {
        foreach (var pair in _sessions.ToArray())
        {
            if (pair.Key == sessionId)
            {
                continue;
            }

            var session = pair.Value;
            var senderMatches = role == "sender" && session.RegisteredSenderEndpoint is not null && EndpointsEqual(session.RegisteredSenderEndpoint, remoteEndPoint);
            var receiverMatches = role == "receiver" && session.RegisteredReceiverEndpoint is not null && EndpointsEqual(session.RegisteredReceiverEndpoint, remoteEndPoint);
            if (!senderMatches && !receiverMatches)
            {
                continue;
            }

            var updated = session with
            {
                RegisteredSenderEndpoint = senderMatches ? null : session.RegisteredSenderEndpoint,
                RegisteredReceiverEndpoint = receiverMatches ? null : session.RegisteredReceiverEndpoint,
            };

            if (updated.RegisteredSenderEndpoint is null && updated.RegisteredReceiverEndpoint is null)
            {
                _sessions.Remove(pair.Key);
                Console.WriteLine(
                    $"Relay removed stale session mapping {pair.Key} for {remoteEndPoint.Address}:{remoteEndPoint.Port} (role={role})");
            }
            else
            {
                _sessions[pair.Key] = updated;
                Console.WriteLine(
                    $"Relay pruned stale endpoint from session {pair.Key} for {remoteEndPoint.Address}:{remoteEndPoint.Port} (role={role})");
            }
        }
    }

    private bool TryResolveForwardTarget(IPEndPoint source, DateTimeOffset now, out IPEndPoint target)
    {
        lock (_sync)
        {
            RelaySessionRuntime? bestSession = null;
            string? bestSessionId = null;
            string matchedRole = string.Empty;

            foreach (var pair in _sessions.ToArray())
            {
                var session = pair.Value;
                if (session.RegisteredSenderEndpoint is not null && EndpointsEqual(session.RegisteredSenderEndpoint, source))
                {
                    if (bestSession is null || session.RegisteredSenderLastSeenUtc > bestSession.RegisteredSenderLastSeenUtc)
                    {
                        bestSession = session;
                        bestSessionId = pair.Key;
                        matchedRole = "sender";
                    }
                }

                if (session.RegisteredReceiverEndpoint is not null && EndpointsEqual(session.RegisteredReceiverEndpoint, source))
                {
                    if (bestSession is null || session.RegisteredReceiverLastSeenUtc > bestSession.RegisteredReceiverLastSeenUtc)
                    {
                        bestSession = session;
                        bestSessionId = pair.Key;
                        matchedRole = "receiver";
                    }
                }
            }

            if (bestSession is not null && bestSessionId is not null)
            {
                if (matchedRole == "sender")
                {
                    var updated = bestSession with { RegisteredSenderLastSeenUtc = now };
                    _sessions[bestSessionId] = updated;
                    if (updated.RegisteredReceiverEndpoint is not null && now - updated.RegisteredReceiverLastSeenUtc <= RegistrationTtl)
                    {
                        target = updated.RegisteredReceiverEndpoint;
                        return true;
                    }
                }
                else if (matchedRole == "receiver")
                {
                    var updated = bestSession with { RegisteredReceiverLastSeenUtc = now };
                    _sessions[bestSessionId] = updated;
                    if (updated.RegisteredSenderEndpoint is not null && now - updated.RegisteredSenderLastSeenUtc <= RegistrationTtl)
                    {
                        target = updated.RegisteredSenderEndpoint;
                        return true;
                    }
                }
            }
        }

        target = new IPEndPoint(IPAddress.Any, 0);
        return false;
    }

    private void CleanupExpiredSessions(DateTimeOffset now)
    {
        lock (_sync)
        {
            foreach (var session in _sessions.Values.ToArray())
            {
                var senderAlive = session.RegisteredSenderEndpoint is not null && now - session.RegisteredSenderLastSeenUtc <= RegistrationTtl;
                var receiverAlive = session.RegisteredReceiverEndpoint is not null && now - session.RegisteredReceiverLastSeenUtc <= RegistrationTtl;
                if (!senderAlive && !receiverAlive)
                {
                    _sessions.Remove(session.SessionId);
                    continue;
                }

                if (!senderAlive || !receiverAlive)
                {
                    _sessions[session.SessionId] = session with
                    {
                        RegisteredSenderEndpoint = senderAlive ? session.RegisteredSenderEndpoint : null,
                        RegisteredReceiverEndpoint = receiverAlive ? session.RegisteredReceiverEndpoint : null,
                    };
                }
            }
        }
    }

    private async Task HeartbeatLoopAsync(CancellationToken cancellationToken)
    {
        using var timer = new PeriodicTimer(TimeSpan.FromSeconds(5));
        while (await timer.WaitForNextTickAsync(cancellationToken))
        {
            try
            {
                await EnsureRegisteredAsync(cancellationToken);
                await SendHeartbeatAsync(cancellationToken);
            }
            catch (OperationCanceledException)
            {
                return;
            }
            catch (Exception ex)
            {
                Console.WriteLine($"Relay heartbeat failed: {ex.Message}");
            }
        }
    }

    private async Task EnsureRegisteredAsync(CancellationToken cancellationToken)
    {
        var request = new RegisterRelayRequest(
            RelayId: _registrationState?.RelayId,
            RelaySecret: _registrationState?.RelaySecret,
            DisplayName: _settings.DisplayName,
            Region: _settings.Region,
            PublicAddress: _settings.PublicAddress,
            UdpPort: _settings.UdpPort,
            Availability: 1);

        using var response = await _httpClient.PostAsJsonAsync(
            BuildUri(_settings.ControlPlaneBaseUrl, "/api/relay/register"),
            request,
            _jsonOptions,
            cancellationToken);
        response.EnsureSuccessStatusCode();

        var payload = await response.Content.ReadFromJsonAsync<RegisterRelayResponse>(_jsonOptions, cancellationToken)
            ?? throw new InvalidOperationException("Relay register returned empty response.");

        _registrationState = new RelayRegistrationState(payload.RelayId, payload.RelaySecret, payload.HeartbeatIntervalSeconds);
        SavePersistedState(_registrationState);
    }

    private async Task SendHeartbeatAsync(CancellationToken cancellationToken)
    {
        if (_registrationState is null)
        {
            return;
        }

        var request = new RelayHeartbeatRequest(
            RelaySecret: _registrationState.RelaySecret,
            Availability: 1,
            PublicAddress: _settings.PublicAddress,
            UdpPort: _settings.UdpPort);

        using var response = await _httpClient.PostAsJsonAsync(
            BuildUri(_settings.ControlPlaneBaseUrl, $"/api/relay/{_registrationState.RelayId}/heartbeat"),
            request,
            _jsonOptions,
            cancellationToken);

        if (response.StatusCode is HttpStatusCode.NotFound or HttpStatusCode.Unauthorized)
        {
            _registrationState = null;
            ClearPersistedState();
            await EnsureRegisteredAsync(cancellationToken);
            return;
        }

        response.EnsureSuccessStatusCode();
    }

    private RelayRegistrationState? LoadPersistedState()
    {
        try
        {
            if (!File.Exists(_statePath))
            {
                return null;
            }

            var payload = JsonSerializer.Deserialize<PersistedRelayState>(File.ReadAllText(_statePath), _jsonOptions);
            if (payload is null || !string.Equals(payload.BaseUrl, _settings.ControlPlaneBaseUrl, StringComparison.OrdinalIgnoreCase))
            {
                return null;
            }

            if (string.IsNullOrWhiteSpace(payload.RelayId) || string.IsNullOrWhiteSpace(payload.RelaySecret))
            {
                return null;
            }

            return new RelayRegistrationState(payload.RelayId, payload.RelaySecret, payload.HeartbeatIntervalSeconds);
        }
        catch
        {
            return null;
        }
    }

    private void SavePersistedState(RelayRegistrationState state)
    {
        try
        {
            var payload = new PersistedRelayState(_settings.ControlPlaneBaseUrl, state.RelayId, state.RelaySecret, state.HeartbeatIntervalSeconds);
            File.WriteAllText(_statePath, JsonSerializer.Serialize(payload, _jsonOptions));
        }
        catch
        {
        }
    }

    private void ClearPersistedState()
    {
        try
        {
            if (File.Exists(_statePath))
            {
                File.Delete(_statePath);
            }
        }
        catch
        {
        }
    }

    private static bool EndpointsEqual(IPEndPoint left, IPEndPoint right) =>
        left.Port == right.Port && left.Address.Equals(right.Address);

    private static bool FixedTimeEquals(string left, string right)
    {
        var leftBytes = System.Text.Encoding.UTF8.GetBytes(left);
        var rightBytes = System.Text.Encoding.UTF8.GetBytes(right);
        return leftBytes.Length == rightBytes.Length && CryptographicOperations.FixedTimeEquals(leftBytes, rightBytes);
    }

    private static Uri BuildUri(string baseUrl, string relativePath) =>
        new($"{baseUrl.Trim().TrimEnd('/')}{relativePath}", UriKind.Absolute);
}

sealed record RelayNodeSettings(
    string ControlPlaneBaseUrl,
    string DisplayName,
    string Region,
    string PublicAddress,
    int UdpPort)
{
    public static RelayNodeSettings FromArgs(string[] args)
    {
        string GetOption(string name, string environmentName, string fallback)
        {
            for (var index = 0; index < args.Length - 1; index++)
            {
                if (string.Equals(args[index], name, StringComparison.OrdinalIgnoreCase))
                {
                    return args[index + 1];
                }
            }

            var environmentValue = Environment.GetEnvironmentVariable(environmentName);
            if (!string.IsNullOrWhiteSpace(environmentValue))
            {
                return environmentValue;
            }

            return fallback;
        }

        var udpPortText = GetOption("--udp-port", "EVERTY_RELAY_UDP_PORT", "6200");
        var udpPort = int.TryParse(udpPortText, out var parsedPort) && parsedPort is > 0 and <= 65535
            ? parsedPort
            : 6200;

        return new RelayNodeSettings(
            ControlPlaneBaseUrl: GetOption("--control-plane", "EVERTY_RELAY_CONTROL_PLANE", "http://127.0.0.1:5180").Trim().TrimEnd('/'),
            DisplayName: GetOption("--display-name", "EVERTY_RELAY_DISPLAY_NAME", $"{Environment.MachineName} Relay"),
            Region: GetOption("--region", "EVERTY_RELAY_REGION", "global"),
            PublicAddress: GetOption("--public-address", "EVERTY_RELAY_PUBLIC_ADDRESS", "127.0.0.1"),
            UdpPort: udpPort);
    }
}

sealed record RelayRegistrationState(string RelayId, string RelaySecret, int HeartbeatIntervalSeconds);

sealed record PersistedRelayState(string BaseUrl, string RelayId, string RelaySecret, int HeartbeatIntervalSeconds);

sealed record RelaySessionRuntime(
    string SessionId,
    string SessionToken,
    IPEndPoint? RegisteredSenderEndpoint,
    DateTimeOffset RegisteredSenderLastSeenUtc,
    IPEndPoint? RegisteredReceiverEndpoint,
    DateTimeOffset RegisteredReceiverLastSeenUtc,
    IPEndPoint? ControlPlaneSenderEndpoint,
    IPEndPoint? ControlPlaneReceiverEndpoint);

sealed record RegisterRelayRequest(
    string? RelayId,
    string? RelaySecret,
    string DisplayName,
    string Region,
    string PublicAddress,
    int UdpPort,
    int Availability);

sealed record RegisterRelayResponse(
    string RelayId,
    string RelaySecret,
    int HeartbeatIntervalSeconds);

sealed record StreamEndpoint(
    string Host,
    int Port,
    string Transport);

sealed record SessionConnectSnapshot(
    StreamEndpoint? StreamEndpoint,
    StreamEndpoint? ReceiverEndpoint,
    string? RouteKind,
    JsonElement? Status);

sealed record RelayRegistrationAck(
    string Kind,
    string SessionId,
    string Role,
    string ObservedAddress,
    int ObservedPort);

sealed record RelaySessionRegistrationRequest(
    string SessionToken,
    string Role,
    string ObservedAddress,
    int ObservedPort);

sealed record RelayHeartbeatRequest(
    string RelaySecret,
    int Availability,
    string PublicAddress,
    int UdpPort);

sealed record RelayRegistrationMessage(
    string SessionId,
    string SessionToken,
    string Role);

sealed record NatProbeRequest(
    string SessionId,
    string ProbeToken,
    string Role);

sealed record NatProbeResponse(
    string Kind,
    string SessionId,
    string ProbeToken,
    string ObservedAddress,
    int ObservedPort);

static class RelayProtocol
{
    private const int Magic = 0x45565254;
    private const byte Version = 3;
    private const byte TypeControl = 4;
    private const int HeaderSize = 24;

    public static bool TryParseNatProbe(byte[] datagram, int length, out NatProbeRequest? probe)
    {
        probe = null;
        if (length <= 0 || datagram[0] != (byte)'{')
        {
            return false;
        }

        try
        {
            using var document = JsonDocument.Parse(datagram.AsMemory(0, length));
            if (!document.RootElement.TryGetProperty("kind", out var kindProperty) ||
                !string.Equals(kindProperty.GetString(), "nat_probe", StringComparison.OrdinalIgnoreCase))
            {
                return false;
            }

            var sessionId = document.RootElement.TryGetProperty("sessionId", out var sessionIdProperty)
                ? sessionIdProperty.GetString()
                : null;
            var probeToken = document.RootElement.TryGetProperty("probeToken", out var probeTokenProperty)
                ? probeTokenProperty.GetString()
                : null;
            var role = document.RootElement.TryGetProperty("role", out var roleProperty)
                ? roleProperty.GetString()
                : null;
            if (string.IsNullOrWhiteSpace(sessionId) || string.IsNullOrWhiteSpace(probeToken))
            {
                return false;
            }

            probe = new NatProbeRequest(
                SessionId: sessionId.Trim(),
                ProbeToken: probeToken.Trim(),
                Role: string.IsNullOrWhiteSpace(role) ? "client" : role.Trim().ToLowerInvariant());
            return true;
        }
        catch
        {
            return false;
        }
    }

    public static bool TryParseRelayRegistration(byte[] datagram, int length, out RelayRegistrationMessage? registration)
    {
        registration = null;
        if (length < HeaderSize)
        {
            return false;
        }

        var span = datagram.AsSpan(0, length);
        if (BinaryPrimitives.ReadInt32BigEndian(span[..4]) != Magic ||
            span[4] != Version ||
            span[5] != TypeControl)
        {
            return false;
        }

        try
        {
            using var document = JsonDocument.Parse(span[HeaderSize..].ToArray());
            if (!document.RootElement.TryGetProperty("kind", out var kindProperty) ||
                !string.Equals(kindProperty.GetString(), "relay_register", StringComparison.OrdinalIgnoreCase))
            {
                return false;
            }

            var sessionId = document.RootElement.TryGetProperty("sessionId", out var sessionIdProperty)
                ? sessionIdProperty.GetString()
                : null;
            var sessionToken = document.RootElement.TryGetProperty("sessionToken", out var sessionTokenProperty)
                ? sessionTokenProperty.GetString()
                : null;
            var role = document.RootElement.TryGetProperty("role", out var roleProperty)
                ? roleProperty.GetString()
                : null;

            if (string.IsNullOrWhiteSpace(sessionId) || string.IsNullOrWhiteSpace(sessionToken) || string.IsNullOrWhiteSpace(role))
            {
                return false;
            }

            registration = new RelayRegistrationMessage(
                SessionId: sessionId.Trim(),
                SessionToken: sessionToken.Trim(),
                Role: role.Trim().ToLowerInvariant());
            return true;
        }
        catch
        {
            return false;
        }
    }

    public static bool TryDescribeControlPacket(byte[] datagram, int length, out string description)
    {
        description = string.Empty;
        if (length < HeaderSize)
        {
            description = $"short:{length}";
            return false;
        }

        var span = datagram.AsSpan(0, length);
        var magic = BinaryPrimitives.ReadInt32BigEndian(span[..4]);
        var version = span[4];
        var type = span[5];
        if (magic != Magic)
        {
            description = $"magic=0x{magic:X8}";
            return false;
        }

        if (type != TypeControl)
        {
            description = $"version={version};type={type}";
            return true;
        }

        try
        {
            using var document = JsonDocument.Parse(span[HeaderSize..].ToArray());
            var kind = document.RootElement.TryGetProperty("kind", out var kindProperty)
                ? kindProperty.GetString()
                : null;
            description = $"version={version};type={type};kind={kind ?? "-"}";
            return true;
        }
        catch (Exception ex)
        {
            description = $"version={version};type={type};json-error={ex.GetType().Name}";
            return true;
        }
    }

    public static byte[] BuildControlPacket(byte[] payload)
    {
        var packet = new byte[HeaderSize + payload.Length];
        var span = packet.AsSpan();
        BinaryPrimitives.WriteInt32BigEndian(span[..4], Magic);
        span[4] = Version;
        span[5] = TypeControl;
        BinaryPrimitives.WriteUInt16BigEndian(span.Slice(6, 2), 0);
        BinaryPrimitives.WriteInt32BigEndian(span.Slice(8, 4), 0);
        BinaryPrimitives.WriteUInt16BigEndian(span.Slice(12, 2), 0);
        BinaryPrimitives.WriteUInt16BigEndian(span.Slice(14, 2), 1);
        BinaryPrimitives.WriteInt64BigEndian(span.Slice(16, 8), 0L);
        payload.CopyTo(span[HeaderSize..]);
        return packet;
    }
}
