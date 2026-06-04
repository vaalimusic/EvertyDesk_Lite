namespace ReceiverNative;

internal enum WindowsSenderEncoderBackend
{
    Auto,
    NvidiaNvencNative,
    MediaFoundation,
    NvidiaNvenc,
    IntelQuickSync,
    FfmpegSoftware,
}

internal static class WindowsSenderEncoderBackendExtensions
{
    public static string ToUiLabel(this WindowsSenderEncoderBackend backend) =>
        backend switch
        {
            WindowsSenderEncoderBackend.Auto => "Auto",
            WindowsSenderEncoderBackend.NvidiaNvencNative => "NVIDIA NVENC Native",
            WindowsSenderEncoderBackend.MediaFoundation => "Media Foundation",
            WindowsSenderEncoderBackend.NvidiaNvenc => "NVIDIA NVENC (FFmpeg)",
            WindowsSenderEncoderBackend.IntelQuickSync => "Intel Quick Sync",
            WindowsSenderEncoderBackend.FfmpegSoftware => "FFmpeg Software",
            _ => throw new ArgumentOutOfRangeException(nameof(backend), backend, null),
        };

    public static string ToPathLabel(this WindowsSenderEncoderBackend backend) =>
        backend switch
        {
            WindowsSenderEncoderBackend.Auto => "Auto",
            WindowsSenderEncoderBackend.NvidiaNvencNative => "NVIDIA NVENC native",
            WindowsSenderEncoderBackend.MediaFoundation => "Media Foundation native",
            WindowsSenderEncoderBackend.NvidiaNvenc => "FFmpeg NVIDIA NVENC",
            WindowsSenderEncoderBackend.IntelQuickSync => "FFmpeg Intel Quick Sync",
            WindowsSenderEncoderBackend.FfmpegSoftware => "FFmpeg software fallback",
            _ => throw new ArgumentOutOfRangeException(nameof(backend), backend, null),
        };
}
