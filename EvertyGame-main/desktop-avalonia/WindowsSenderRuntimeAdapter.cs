using System.Collections.Generic;
using ReceiverNative;

namespace Everty.Desktop.Avalonia;

internal sealed class WindowsSenderRuntimeAdapter : IWindowsSenderRuntime
{
    private readonly WindowsSenderSession _session = new();

    public void Dispose() => _session.Dispose();

    public IReadOnlyList<WindowsCaptureTargetInfo> GetCaptureTargets() => WindowsSenderSession.GetCaptureTargets();

    public WindowsSenderSession.SenderCapabilityProbeResult GetCapabilityProbe() => WindowsSenderSession.GetSenderCapabilityProbe();

    public WindowsSenderSessionSnapshot GetSnapshot() => _session.GetSnapshot();

    public void Start(string host, int port, string captureTargetDeviceName, WindowsSenderEncoderBackend encoderBackend, WindowsVideoCodec codec, WindowsSenderPresetSpec spec, bool audioEnabled, bool captureCursorInStream, bool latencyPulseFlashEnabled, bool adaptiveEnabled, RelayTransportRoute? relayRoute) =>
        _session.Start(host, port, captureTargetDeviceName, encoderBackend, codec, spec, audioEnabled, captureCursorInStream, latencyPulseFlashEnabled, adaptiveEnabled, relayRoute);

    public void Stop() => _session.Stop();
}
