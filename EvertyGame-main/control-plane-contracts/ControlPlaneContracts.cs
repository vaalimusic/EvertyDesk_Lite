using System.Globalization;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace Everty.ControlPlane.Contracts;

public sealed class HostAvailabilityJsonConverter : JsonConverter<string>
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

public sealed class SessionStatusJsonConverter : JsonConverter<string>
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

public sealed record DesktopControlPlaneHostSummary(
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
            return "Off";
        }

        return !string.IsNullOrWhiteSpace(ActiveSessionId)
            ? "Busy"
            : "Available";
    }
}

public sealed record DesktopControlPlaneDesiredStreamRequest(
    int? Width,
    int? Height,
    int? Fps,
    int? BitrateBps,
    bool? CaptureCursor,
    bool? AdaptiveMode,
    IReadOnlyList<string>? PreferredCodecs = null,
    string? PresetId = null);

public sealed record DesktopControlPlaneClientCapabilities(
    IReadOnlyList<string>? SupportedDecodeCodecs = null,
    IReadOnlyList<string>? LanAddresses = null);

public sealed record DesktopControlPlaneSessionLease(
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

public sealed record DesktopControlPlaneConnectInstructions(
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

public sealed record DesktopControlPlaneRoutePolicy(
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

public sealed record DesktopControlPlaneAuthState(
    string Mode,
    string Label,
    bool UserAuthenticated);

public sealed record DesktopControlPlaneManagedSessionState(
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
