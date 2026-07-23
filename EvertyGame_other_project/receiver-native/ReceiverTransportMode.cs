namespace ReceiverNative;

internal enum ReceiverTransportMode
{
    Udp,
    AdbTunnelTcp,
}

internal static class ReceiverTransportModeExtensions
{
    public static string ToUiLabel(this ReceiverTransportMode mode)
    {
        return mode switch
        {
            ReceiverTransportMode.Udp => "UDP / LAN",
            ReceiverTransportMode.AdbTunnelTcp => "ADB tunnel / TCP",
            _ => mode.ToString(),
        };
    }

    public static string ToPortLabel(this ReceiverTransportMode mode)
    {
        return mode switch
        {
            ReceiverTransportMode.Udp => "UDP",
            ReceiverTransportMode.AdbTunnelTcp => "TCP",
            _ => "Port",
        };
    }

    public static string BuildWaitingStatus(this ReceiverTransportMode mode, int port)
    {
        return mode switch
        {
            ReceiverTransportMode.Udp => $"Listening on UDP {port}. Waiting for sender",
            ReceiverTransportMode.AdbTunnelTcp => $"Listening on TCP {port}. Run adb reverse tcp:{port} tcp:{port}, then connect sender to 127.0.0.1",
            _ => $"Listening on {port}",
        };
    }
}
