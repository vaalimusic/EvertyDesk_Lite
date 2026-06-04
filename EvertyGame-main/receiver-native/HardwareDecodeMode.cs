namespace ReceiverNative;

internal enum HardwareDecodeMode
{
    Auto,
    D3D11VA,
    NvidiaLowLatency,
    IntelQuickSync,
    DXVA2,
    Disabled,
}

internal static class HardwareDecodeModeExtensions
{
    public static string ToUiLabel(this HardwareDecodeMode mode)
    {
        return mode switch
        {
            HardwareDecodeMode.Auto => "Auto",
            HardwareDecodeMode.D3D11VA => "D3D11VA",
            HardwareDecodeMode.NvidiaLowLatency => "NVIDIA Low Latency",
            HardwareDecodeMode.IntelQuickSync => "Intel QuickSync",
            HardwareDecodeMode.DXVA2 => "DXVA2",
            HardwareDecodeMode.Disabled => "Disabled",
            _ => mode.ToString(),
        };
    }

    public static string ToVlcOption(this HardwareDecodeMode mode)
    {
        return mode switch
        {
            HardwareDecodeMode.Auto => "any",
            HardwareDecodeMode.D3D11VA => "d3d11va",
            HardwareDecodeMode.NvidiaLowLatency => "d3d11va",
            HardwareDecodeMode.IntelQuickSync => "qsv",
            HardwareDecodeMode.DXVA2 => "dxva2",
            HardwareDecodeMode.Disabled => "none",
            _ => "any",
        };
    }

    public static bool IsNvidiaLowLatencyProfile(this HardwareDecodeMode mode)
    {
        return mode == HardwareDecodeMode.NvidiaLowLatency;
    }

    public static bool IsIntelQuickSync(this HardwareDecodeMode mode)
    {
        return mode == HardwareDecodeMode.IntelQuickSync;
    }
}
