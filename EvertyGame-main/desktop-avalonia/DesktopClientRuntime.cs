using System;
using System.Net;
using System.Net.NetworkInformation;
using System.Net.Sockets;
using System.Windows.Forms;
using ReceiverNative;

namespace Everty.Desktop.Avalonia;

internal sealed class DesktopClientRuntime : IDisposable
{
    private readonly object _sync = new();
    private DesktopClientPlaybackForm? _playbackForm;
    private NativeReceiverSession? _session;
    private int _listenPort;

    public ReceiverSessionSnapshot GetSnapshot()
    {
        lock (_sync)
        {
            return _session?.GetSnapshot() ?? new ReceiverSessionSnapshot();
        }
    }

    public int ListenPort
    {
        get
        {
            lock (_sync)
            {
                return _listenPort;
            }
        }
    }

    public bool EnsureReceiverEndpoint(int requestedPort, out string host, out int port)
    {
        host = ResolvePreferredIpv4Address();
        port = requestedPort is > 0 and <= 65535 ? requestedPort : 5001;
        if (string.IsNullOrWhiteSpace(host))
        {
            return false;
        }

        EnsureSessionCreated();
        return true;
    }

    public void Start(int port, RelayTransportRoute? relayRegistrationRoute, RelayTransportRoute? relayRoute)
    {
        lock (_sync)
        {
            EnsureSessionCreated();
            _listenPort = port;
            _session!.ConfigureRelayRegistrationRoute(relayRegistrationRoute);
            _session.ConfigureRelayRoute(relayRoute);
            _session.Start(port, ReceiverTransportMode.Udp, HardwareDecodeMode.Auto, aggressiveMode: true);
            _playbackForm!.Show();
            if (_playbackForm.WindowState == FormWindowState.Minimized)
            {
                _playbackForm.WindowState = FormWindowState.Normal;
            }

            _playbackForm.BringToFront();
            _playbackForm.ApplySnapshot(_session.GetSnapshot());
        }
    }

    public void Stop()
    {
        lock (_sync)
        {
            _session?.ConfigureRelayRoute(null);
            _session?.ConfigureRelayRegistrationRoute(null);
            _session?.Stop();
            _listenPort = 0;
            if (_playbackForm is not null && !_playbackForm.IsDisposed)
            {
                _playbackForm.Hide();
                _playbackForm.ApplySnapshot(new ReceiverSessionSnapshot());
            }
        }
    }

    public void RefreshPlaybackWindow()
    {
        lock (_sync)
        {
            if (_session is null || _playbackForm is null || _playbackForm.IsDisposed)
            {
                return;
            }

            _playbackForm.ApplySnapshot(_session.GetSnapshot());
        }
    }

    public bool HasActivePlaybackWindow
    {
        get
        {
            lock (_sync)
            {
                return _playbackForm is not null && !_playbackForm.IsDisposed;
            }
        }
    }

    public bool IsPlaybackWindowVisible
    {
        get
        {
            lock (_sync)
            {
                return _playbackForm is not null && !_playbackForm.IsDisposed && _playbackForm.Visible;
            }
        }
    }

    public void ShowPlaybackWindow()
    {
        lock (_sync)
        {
            if (_playbackForm is null || _playbackForm.IsDisposed)
            {
                return;
            }

            _playbackForm.Show();
            if (_playbackForm.WindowState == FormWindowState.Minimized)
            {
                _playbackForm.WindowState = FormWindowState.Normal;
            }

            _playbackForm.BringToFront();
            _playbackForm.ApplySnapshot(_session?.GetSnapshot() ?? new ReceiverSessionSnapshot());
        }
    }

    public void HidePlaybackWindow()
    {
        lock (_sync)
        {
            if (_playbackForm is null || _playbackForm.IsDisposed)
            {
                return;
            }

            _playbackForm.Hide();
        }
    }

    public void Dispose()
    {
        lock (_sync)
        {
            _session?.Dispose();
            _session = null;
            if (_playbackForm is not null && !_playbackForm.IsDisposed)
            {
                _playbackForm.Close();
                _playbackForm.Dispose();
            }

            _playbackForm = null;
        }
    }

    private void EnsureSessionCreated()
    {
        if (_session is not null && _playbackForm is not null && !_playbackForm.IsDisposed)
        {
            return;
        }

        _playbackForm = new DesktopClientPlaybackForm();
        _session = new NativeReceiverSession(_playbackForm.PlaybackHost);
    }

    private static string ResolvePreferredIpv4Address()
    {
        foreach (var networkInterface in NetworkInterface.GetAllNetworkInterfaces())
        {
            if (networkInterface.OperationalStatus != OperationalStatus.Up ||
                networkInterface.NetworkInterfaceType == NetworkInterfaceType.Loopback)
            {
                continue;
            }

            IPInterfaceProperties? properties;
            try
            {
                properties = networkInterface.GetIPProperties();
            }
            catch
            {
                continue;
            }

            foreach (var unicast in properties.UnicastAddresses)
            {
                if (unicast.Address.AddressFamily == AddressFamily.InterNetwork &&
                    !IPAddress.IsLoopback(unicast.Address))
                {
                    return unicast.Address.ToString();
                }
            }
        }

        return string.Empty;
    }
}
