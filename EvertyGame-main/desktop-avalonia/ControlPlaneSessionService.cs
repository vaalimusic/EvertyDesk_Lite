using System;
using System.Collections.Generic;
using System.Linq;
using System.Threading;
using System.Threading.Tasks;
using CPC = Everty.ControlPlane.Contracts;
using RN = ReceiverNative;

namespace Everty.Desktop.Avalonia;

internal sealed class ControlPlaneSessionService : IControlPlaneSessionService
{
    private readonly RN.DesktopControlPlaneClient _client = new();

    public void Dispose() => _client.Dispose();

    public async Task<IReadOnlyList<CPC.DesktopControlPlaneHostSummary>> ListHostsAsync(string baseUrl, CancellationToken cancellationToken = default) =>
        (await _client.ListHostsAsync(baseUrl, cancellationToken))
            .Select(MapHostSummary)
            .ToArray();

    public async Task<CPC.DesktopControlPlaneAuthState> LoginUserAsync(string baseUrl, string email, string password, CancellationToken cancellationToken = default) =>
        MapAuthState(await _client.LoginUserAsync(baseUrl, email, password, cancellationToken));

    public async Task<CPC.DesktopControlPlaneSessionLease> CreateSessionAsync(string baseUrl, string hostId, string clientLabel, string clientRegion, string? codecPreference, bool preferRelay, bool audioRequested, int controllerCount, int leaseMinutes, string receiverAddress, int receiverPort, CPC.DesktopControlPlaneDesiredStreamRequest desiredStream, CPC.DesktopControlPlaneClientCapabilities? clientCapabilities = null, CancellationToken cancellationToken = default) =>
        MapSessionLease(await _client.CreateSessionAsync(baseUrl, hostId, clientLabel, clientRegion, codecPreference, preferRelay, audioRequested, controllerCount, leaseMinutes, receiverAddress, receiverPort, MapDesiredStream(desiredStream), MapClientCapabilities(clientCapabilities), cancellationToken));

    public async Task<CPC.DesktopControlPlaneConnectInstructions> GetConnectInstructionsAsync(string baseUrl, string sessionId, string sessionToken, CancellationToken cancellationToken = default) =>
        MapConnectInstructions(await _client.GetConnectInstructionsAsync(baseUrl, sessionId, sessionToken, cancellationToken));

    public async Task<CPC.DesktopControlPlaneConnectInstructions> ResumeManagedSessionAsync(string baseUrl, string sessionId, string sessionToken, CancellationToken cancellationToken = default) =>
        MapConnectInstructions(await _client.ResumeManagedSessionAsync(baseUrl, sessionId, sessionToken, cancellationToken));

    public Task StopSessionAsync(string baseUrl, string sessionId, string sessionToken, string reason, CancellationToken cancellationToken = default) =>
        _client.StopSessionAsync(baseUrl, sessionId, sessionToken, reason, cancellationToken);

    public CPC.DesktopControlPlaneManagedSessionState? GetManagedSessionState(string baseUrl)
    {
        var state = _client.GetManagedSessionState(baseUrl);
        return state is null ? null : MapManagedSession(state);
    }

    public void SaveManagedSessionState(CPC.DesktopControlPlaneManagedSessionState managedSession) =>
        _client.SaveManagedSessionState(MapManagedSession(managedSession));

    public void ClearManagedSessionState(string baseUrl) => _client.ClearManagedSessionState(baseUrl);

    private static CPC.DesktopControlPlaneHostSummary MapHostSummary(RN.DesktopControlPlaneHostSummary host) =>
        new(host.HostId, host.HostCode, host.DisplayName, host.Region, host.Online, host.Availability, host.ActiveSessionId, host.SupportsHevc, host.SupportsAudio, host.SupportsGamepad, host.PricePerHour, host.Currency, host.Description);

    private static CPC.DesktopControlPlaneAuthState MapAuthState(RN.DesktopControlPlaneAuthState auth) =>
        new(auth.Mode, auth.Label, auth.UserAuthenticated);

    private static RN.DesktopControlPlaneDesiredStreamRequest MapDesiredStream(CPC.DesktopControlPlaneDesiredStreamRequest request) =>
        new(request.Width, request.Height, request.Fps, request.BitrateBps, request.CaptureCursor, request.AdaptiveMode, request.PreferredCodecs, request.PresetId);

    private static RN.DesktopControlPlaneClientCapabilities? MapClientCapabilities(CPC.DesktopControlPlaneClientCapabilities? capabilities) =>
        capabilities is null
            ? null
            : new RN.DesktopControlPlaneClientCapabilities(capabilities.SupportedDecodeCodecs, capabilities.LanAddresses);

    private static CPC.DesktopControlPlaneSessionLease MapSessionLease(RN.DesktopControlPlaneSessionLease lease) =>
        new(lease.SessionId, lease.SessionToken, lease.HostId, lease.HostDisplayName, lease.Status, lease.RouteKind, lease.RouteState, lease.SessionHealth, lease.SessionHealthReason, lease.RouteActionHint, lease.RouteActionReason, lease.RouteFallbackReadyDurationSeconds, lease.RouteRecoveryReadyDurationSeconds, lease.RecommendedSyncDelaySeconds, lease.TransportLossLevel, lease.TransportAnomalyKind, lease.TransportAnomalyReason, lease.TransportAnomalyConfidence, lease.ReceiverTelemetryAgeSeconds, lease.SenderTelemetryAgeSeconds, lease.RouteRecoveryCount, lease.RouteRecoveryCooldownSeconds, lease.RouteFallbackCount, lease.RouteFallbackCooldownSeconds, lease.CodecPreference, lease.RouteVersion, lease.RelayAddress, lease.RelayPort, lease.RelayRegion, lease.ProbeAddress, lease.ProbePort, lease.ProbeToken, lease.NatStatus, lease.HostNatProbeAgeSeconds, lease.ClientNatProbeAgeSeconds, lease.NatProbeFresh, lease.ReceiverAddress, lease.ReceiverPort, lease.LastRouteActionKind, lease.LastRouteActionReason, lease.LastRouteActionActor, lease.LastRouteActionUtc);

    private static CPC.DesktopControlPlaneConnectInstructions MapConnectInstructions(RN.DesktopControlPlaneConnectInstructions instructions) =>
        new(instructions.SessionId, instructions.HostId, instructions.HostDisplayName, instructions.Status, instructions.RouteKind, instructions.RouteState, instructions.RouteVersion, instructions.SessionHealth, instructions.SessionHealthReason, instructions.RouteActionHint, instructions.RouteActionReason, instructions.RouteFallbackReadyDurationSeconds, instructions.RouteRecoveryReadyDurationSeconds, instructions.RecommendedSyncDelaySeconds, instructions.TransportLossLevel, instructions.TransportAnomalyKind, instructions.TransportAnomalyReason, instructions.TransportAnomalyConfidence, instructions.ReceiverTelemetryAgeSeconds, instructions.SenderTelemetryAgeSeconds, instructions.RouteRecoveryCount, instructions.RouteRecoveryCooldownSeconds, instructions.RouteFallbackCount, instructions.RouteFallbackCooldownSeconds, instructions.StreamHost, instructions.StreamPort, instructions.RelayHost, instructions.RelayPort, instructions.RelayRegion, instructions.ProbeHost, instructions.ProbePort, instructions.ProbeToken, instructions.NatStatus, instructions.HostNatProbeAgeSeconds, instructions.ClientNatProbeAgeSeconds, instructions.NatProbeFresh, instructions.ReceiverHost, instructions.ReceiverPort, instructions.LastRouteActionKind, instructions.LastRouteActionReason, instructions.LastRouteActionActor, instructions.LastRouteActionUtc);

    private static CPC.DesktopControlPlaneManagedSessionState MapManagedSession(RN.DesktopControlPlaneManagedSessionState state) =>
        new(state.BaseUrl, state.SessionId, state.SessionToken, state.HostId, state.HostDisplayName, state.RouteKind, state.RouteState, state.SessionHealth, state.SessionHealthReason, state.RouteActionHint, state.RouteActionReason, state.RouteFallbackReadyDurationSeconds, state.RouteRecoveryReadyDurationSeconds, state.RecommendedSyncDelaySeconds, state.TransportLossLevel, state.TransportAnomalyKind, state.TransportAnomalyReason, state.TransportAnomalyConfidence, state.ReceiverTelemetryAgeSeconds, state.SenderTelemetryAgeSeconds, state.RouteRecoveryCount, state.RouteRecoveryCooldownSeconds, state.NatStatus, state.HostNatProbeAgeSeconds, state.ClientNatProbeAgeSeconds, state.NatProbeFresh, state.RelayAddress, state.RelayPort, state.ReceiverAddress, state.ReceiverPort, state.ProbeAddress, state.ProbePort, state.ProbeToken, state.RouteVersion, state.RouteFallbackCount, state.RouteFallbackCooldownSeconds, state.LastRouteActionKind, state.LastRouteActionReason, state.LastRouteActionActor, state.LastRouteActionUtc, state.CodecPreference);

    private static RN.DesktopControlPlaneManagedSessionState MapManagedSession(CPC.DesktopControlPlaneManagedSessionState state) =>
        new(state.BaseUrl, state.SessionId, state.SessionToken, state.HostId, state.HostDisplayName, state.RouteKind, state.RouteState, state.SessionHealth, state.SessionHealthReason, state.RouteActionHint, state.RouteActionReason, state.RouteFallbackReadyDurationSeconds, state.RouteRecoveryReadyDurationSeconds, state.RecommendedSyncDelaySeconds, state.TransportLossLevel, state.TransportAnomalyKind, state.TransportAnomalyReason, state.TransportAnomalyConfidence, state.ReceiverTelemetryAgeSeconds, state.SenderTelemetryAgeSeconds, state.RouteRecoveryCount, state.RouteRecoveryCooldownSeconds, state.NatStatus, state.HostNatProbeAgeSeconds, state.ClientNatProbeAgeSeconds, state.NatProbeFresh, state.RelayAddress, state.RelayPort, state.ReceiverAddress, state.ReceiverPort, state.ProbeAddress, state.ProbePort, state.ProbeToken, state.RouteVersion, state.RouteFallbackCount, state.RouteFallbackCooldownSeconds, state.LastRouteActionKind, state.LastRouteActionReason, state.LastRouteActionActor, state.LastRouteActionUtc, state.CodecPreference);
}
