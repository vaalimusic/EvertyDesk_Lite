using System.Windows.Forms;

namespace ReceiverNative;

internal static class PlaybackControllerFactory
{
    public static IPlaybackController Create(PlaybackBackendKind backendKind, Control playbackHost)
    {
        return backendKind switch
        {
            PlaybackBackendKind.MediaFoundationDirect3D11 => new MediaFoundationD3D11PlaybackController(playbackHost),
            PlaybackBackendKind.LibVlcHwndDirect3D11 => new LibVlcPlaybackController(playbackHost, forceDirect3D11Vout: true, directWindowBinding: true),
            PlaybackBackendKind.LibVlcDirect3D11 => new LibVlcPlaybackController(playbackHost, forceDirect3D11Vout: true),
            PlaybackBackendKind.LibVlcDefault => new LibVlcPlaybackController(playbackHost, forceDirect3D11Vout: false),
            _ => throw new NotSupportedException($"Unsupported playback backend: {backendKind}"),
        };
    }
}
