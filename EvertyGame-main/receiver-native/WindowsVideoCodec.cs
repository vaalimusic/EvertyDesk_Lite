namespace ReceiverNative;

using Vortice.MediaFoundation;

internal enum WindowsVideoCodec
{
    H264Avc,
    H265Hevc,
    Av1,
}

internal static class WindowsVideoCodecExtensions
{
    public static WindowsVideoCodec? TryParse(string? value)
    {
        if (string.IsNullOrWhiteSpace(value))
        {
            return null;
        }

        return value.Trim() switch
        {
            "H.264 / AVC" => WindowsVideoCodec.H264Avc,
            "video/avc" => WindowsVideoCodec.H264Avc,
            "H.265 / HEVC" => WindowsVideoCodec.H265Hevc,
            "video/hevc" => WindowsVideoCodec.H265Hevc,
            "AV1" => WindowsVideoCodec.Av1,
            "video/av1" => WindowsVideoCodec.Av1,
            _ => null,
        };
    }

    public static string ToUiLabel(this WindowsVideoCodec codec) =>
        codec switch
        {
            WindowsVideoCodec.H264Avc => "H.264 / AVC",
            WindowsVideoCodec.H265Hevc => "H.265 / HEVC",
            WindowsVideoCodec.Av1 => "AV1",
            _ => throw new ArgumentOutOfRangeException(nameof(codec), codec, null),
        };

    public static string ToMimeType(this WindowsVideoCodec codec) =>
        codec switch
        {
            WindowsVideoCodec.H264Avc => "video/avc",
            WindowsVideoCodec.H265Hevc => "video/hevc",
            WindowsVideoCodec.Av1 => "video/av1",
            _ => throw new ArgumentOutOfRangeException(nameof(codec), codec, null),
        };

    public static Guid ToMediaFoundationSubtype(this WindowsVideoCodec codec) =>
        codec switch
        {
            WindowsVideoCodec.H264Avc => VideoFormatGuids.H264Es,
            WindowsVideoCodec.H265Hevc => VideoFormatGuids.HevcEs,
            WindowsVideoCodec.Av1 => new Guid("31305641-0000-0010-8000-00AA00389B71"),
            _ => throw new ArgumentOutOfRangeException(nameof(codec), codec, null),
        };

    public static string ToFfmpegEncoderName(this WindowsVideoCodec codec) =>
        codec switch
        {
            WindowsVideoCodec.H264Avc => "libx264",
            WindowsVideoCodec.H265Hevc => "libx265",
            WindowsVideoCodec.Av1 => "libsvtav1",
            _ => throw new ArgumentOutOfRangeException(nameof(codec), codec, null),
        };

    public static string ToFfmpegMuxerName(this WindowsVideoCodec codec) =>
        codec switch
        {
            WindowsVideoCodec.H264Avc => "h264",
            WindowsVideoCodec.H265Hevc => "hevc",
            WindowsVideoCodec.Av1 => "ivf",
            _ => throw new ArgumentOutOfRangeException(nameof(codec), codec, null),
        };

    public static bool IsHevc(this WindowsVideoCodec codec) => codec == WindowsVideoCodec.H265Hevc;
    public static bool IsAv1(this WindowsVideoCodec codec) => codec == WindowsVideoCodec.Av1;
}
