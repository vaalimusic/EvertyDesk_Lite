namespace ReceiverNative;

internal enum PlaybackBackendKind
{
    MediaFoundationDirect3D11,
    LibVlcHwndDirect3D11,
    LibVlcDirect3D11,
    LibVlcDefault,
}

internal static class PlaybackBackendKindExtensions
{
    public static string ToUiLabel(this PlaybackBackendKind kind)
    {
        return kind switch
        {
            PlaybackBackendKind.MediaFoundationDirect3D11 => "Media Foundation + D3D11",
            PlaybackBackendKind.LibVlcHwndDirect3D11 => "LibVLC HWND + D3D11",
            PlaybackBackendKind.LibVlcDirect3D11 => "LibVLC + D3D11",
            PlaybackBackendKind.LibVlcDefault => "LibVLC Default",
            _ => kind.ToString(),
        };
    }
}
