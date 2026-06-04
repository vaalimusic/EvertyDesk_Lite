using System;
using ReceiverNative;

namespace Everty.Desktop.Avalonia;

internal sealed class HostControlPlaneAgentAdapter : IHostControlPlaneAgent
{
    private readonly ControlPlaneAgent _agent = new();

    public event Action<ControlPlaneAgentSnapshot>? SnapshotChanged
    {
        add => _agent.SnapshotChanged += value;
        remove => _agent.SnapshotChanged -= value;
    }

    public ControlPlaneAgentSnapshot GetSnapshot() => _agent.GetSnapshot();

    public void ApplyConfiguration(ControlPlaneAgentConfiguration configuration) => _agent.ApplyConfiguration(configuration);

    public void Dispose() => _agent.Dispose();
}
