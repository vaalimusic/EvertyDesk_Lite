using System;
using System.Collections.ObjectModel;
using System.Collections.Generic;
using System.Threading;
using System.Threading.Tasks;
using CPC = Everty.ControlPlane.Contracts;
using ReceiverNative;

namespace Everty.Desktop.Avalonia;

internal interface IDesktopHostViewModel
{
    string HostStatus { get; }
    string HostDetail { get; }
    string HostSnapshot { get; }
    bool HostRunning { get; }
    Task StartHostAsync();
    Task StopHostAsync();
    Task RefreshHostsAsync();
}

internal interface IDesktopClientViewModel
{
    string ClientStatus { get; }
    string ClientDetail { get; }
    string ClientSession { get; }
    string ClientRoute { get; }
    string ClientEndpoint { get; }
    string ClientCodec { get; }
    string SelectedHostCode { get; set; }
    string SelectedClientPreset { get; set; }
    ObservableCollection<CPC.DesktopControlPlaneHostSummary> Hosts { get; }
    Task LoginDemoAsync(string email, string password);
    Task RefreshHostsAsync();
    Task RestoreManagedSessionAsync();
    Task ConnectByCodeAsync();
    Task StopClientAsync();
    void OpenClientPlaybackWindow();
    void HideClientPlaybackWindow();
}

internal interface IPlaybackSurface
{
    string Title { get; }
    string Description { get; }
}

internal interface IControlPlaneSessionService : IDisposable
{
    Task<IReadOnlyList<CPC.DesktopControlPlaneHostSummary>> ListHostsAsync(string baseUrl, CancellationToken cancellationToken = default);
    Task<CPC.DesktopControlPlaneAuthState> LoginUserAsync(string baseUrl, string email, string password, CancellationToken cancellationToken = default);
    Task<CPC.DesktopControlPlaneSessionLease> CreateSessionAsync(string baseUrl, string hostId, string clientLabel, string clientRegion, string? codecPreference, bool preferRelay, bool audioRequested, int controllerCount, int leaseMinutes, string receiverAddress, int receiverPort, CPC.DesktopControlPlaneDesiredStreamRequest desiredStream, CPC.DesktopControlPlaneClientCapabilities? clientCapabilities = null, CancellationToken cancellationToken = default);
    Task<CPC.DesktopControlPlaneConnectInstructions> GetConnectInstructionsAsync(string baseUrl, string sessionId, string sessionToken, CancellationToken cancellationToken = default);
    Task<CPC.DesktopControlPlaneConnectInstructions> ResumeManagedSessionAsync(string baseUrl, string sessionId, string sessionToken, CancellationToken cancellationToken = default);
    Task StopSessionAsync(string baseUrl, string sessionId, string sessionToken, string reason, CancellationToken cancellationToken = default);
    CPC.DesktopControlPlaneManagedSessionState? GetManagedSessionState(string baseUrl);
    void SaveManagedSessionState(CPC.DesktopControlPlaneManagedSessionState managedSession);
    void ClearManagedSessionState(string baseUrl);
}

internal interface IHostControlPlaneAgent : IDisposable
{
    event Action<ControlPlaneAgentSnapshot>? SnapshotChanged;
    ControlPlaneAgentSnapshot GetSnapshot();
    void ApplyConfiguration(ControlPlaneAgentConfiguration configuration);
}

internal interface IWindowsSenderRuntime : IDisposable
{
    IReadOnlyList<WindowsCaptureTargetInfo> GetCaptureTargets();
    WindowsSenderSession.SenderCapabilityProbeResult GetCapabilityProbe();
    WindowsSenderSessionSnapshot GetSnapshot();
    void Start(string host, int port, string captureTargetDeviceName, WindowsSenderEncoderBackend encoderBackend, WindowsVideoCodec codec, WindowsSenderPresetSpec spec, bool audioEnabled, bool captureCursorInStream, bool latencyPulseFlashEnabled, bool adaptiveEnabled, RelayTransportRoute? relayRoute);
    void Stop();
}
