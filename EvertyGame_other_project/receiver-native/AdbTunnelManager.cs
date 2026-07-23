using System.Diagnostics;

namespace ReceiverNative;

internal static class AdbTunnelManager
{
    public static AdbTunnelResult PrepareReverse(int port)
    {
        if (port is < 1 or > 65535)
        {
            return new AdbTunnelResult(false, $"Invalid port: {port}");
        }

        var adbPath = ResolveAdbPath();
        var startServer = RunAdb(adbPath, "start-server");
        if (!startServer.Success)
        {
            return new AdbTunnelResult(false, startServer.Message);
        }

        var reverse = RunAdb(adbPath, $"reverse tcp:{port} tcp:{port}");
        if (!reverse.Success)
        {
            return new AdbTunnelResult(false, reverse.Message);
        }

        var list = RunAdb(adbPath, "reverse --list");
        var details = list.Success && !string.IsNullOrWhiteSpace(list.Output)
            ? list.Output.Trim()
            : $"reverse tcp:{port} tcp:{port}";
        return new AdbTunnelResult(true, $"Ready: {details}");
    }

    private static string ResolveAdbPath()
    {
        var localAppData = Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData);
        var sdkPath = Path.Combine(localAppData, "Android", "Sdk", "platform-tools", "adb.exe");
        return File.Exists(sdkPath) ? sdkPath : "adb.exe";
    }

    private static CommandResult RunAdb(string adbPath, string arguments)
    {
        try
        {
            using var process = new Process
            {
                StartInfo = new ProcessStartInfo
                {
                    FileName = adbPath,
                    Arguments = arguments,
                    UseShellExecute = false,
                    RedirectStandardOutput = true,
                    RedirectStandardError = true,
                    CreateNoWindow = true,
                },
            };
            process.Start();
            var output = process.StandardOutput.ReadToEnd();
            var error = process.StandardError.ReadToEnd();
            if (!process.WaitForExit(8_000))
            {
                try
                {
                    process.Kill(entireProcessTree: true);
                }
                catch
                {
                }

                return new CommandResult(false, $"adb {arguments} timed out", output);
            }

            if (process.ExitCode == 0)
            {
                return new CommandResult(true, string.Empty, output);
            }

            var message = string.IsNullOrWhiteSpace(error) ? output : error;
            return new CommandResult(false, message.Trim(), output);
        }
        catch (Exception ex)
        {
            return new CommandResult(false, ex.Message, string.Empty);
        }
    }

    private readonly record struct CommandResult(bool Success, string Message, string Output);
}

internal readonly record struct AdbTunnelResult(bool Success, string Message);
