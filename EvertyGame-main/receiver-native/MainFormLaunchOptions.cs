namespace ReceiverNative;

internal sealed record MainFormLaunchOptions(
    AppRole InitialRole,
    string? ControlPlaneUrl = null,
    bool AdvancedMode = false,
    bool LockRoleSelection = false);
