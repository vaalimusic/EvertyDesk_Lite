namespace ReceiverNative;

using System.Net;

internal sealed record RelayTransportRoute(
    string SessionId,
    string SessionToken,
    string RelayHost,
    int RelayPort)
{
    public IPEndPoint ToEndPoint()
    {
        if (IPAddress.TryParse(RelayHost, out var address))
        {
            return new IPEndPoint(address, RelayPort);
        }

        var resolved = Dns.GetHostAddresses(RelayHost)
            .FirstOrDefault(static candidate => candidate.AddressFamily == System.Net.Sockets.AddressFamily.InterNetwork);
        if (resolved is null)
        {
            throw new InvalidOperationException($"Unable to resolve relay host '{RelayHost}'.");
        }

        return new IPEndPoint(resolved, RelayPort);
    }

    public string DisplayText => $"{RelayHost}:{RelayPort}";
}
