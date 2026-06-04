using System.Diagnostics;
using System.Text.RegularExpressions;

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

    public static AdbTunnelResult PrepareShellCapture()
    {
        var adbPath = ResolveAdbPath();
        var ensureDevice = EnsureDeviceReady(adbPath);
        if (!ensureDevice.Success)
        {
            return new AdbTunnelResult(false, ensureDevice.Message);
        }

        var displaySize = QueryPhysicalDisplaySize(adbPath);
        if (!displaySize.Success || displaySize.Profile is null)
        {
            return new AdbTunnelResult(false, displaySize.Message);
        }

        var profile = displaySize.Profile.Value;
        return new AdbTunnelResult(
            true,
            $"Ready: {profile.DisplayWidth}x{profile.DisplayHeight} -> {profile.CaptureWidth}x{profile.CaptureHeight} @ {profile.TargetFps} fps, {(profile.BitrateBps / 1_000_000.0):0.0} Mbps");
    }

    public static string ResolveAdbPath()
    {
        var localAppData = Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData);
        var sdkPath = Path.Combine(localAppData, "Android", "Sdk", "platform-tools", "adb.exe");
        return File.Exists(sdkPath) ? sdkPath : "adb.exe";
    }

    public static AdbTunnelResult EnsureDeviceReady()
    {
        return EnsureDeviceReady(ResolveAdbPath());
    }

    public static AdbScreenrecordProfileResult QueryPhysicalDisplaySize()
    {
        return QueryPhysicalDisplaySize(ResolveAdbPath());
    }

    public static AdbTunnelResult EnsureDeviceReady(string adbPath)
    {
        var startServer = RunAdb(adbPath, "start-server");
        if (!startServer.Success)
        {
            return new AdbTunnelResult(false, startServer.Message);
        }

        var getState = RunAdb(adbPath, "get-state");
        if (!getState.Success)
        {
            return new AdbTunnelResult(false, string.IsNullOrWhiteSpace(getState.Message) ? "adb get-state failed" : getState.Message);
        }

        var state = (getState.Output ?? string.Empty).Trim();
        return string.Equals(state, "device", StringComparison.OrdinalIgnoreCase)
            ? new AdbTunnelResult(true, "ADB device ready")
            : new AdbTunnelResult(false, $"ADB device state is '{state}'");
    }

    public static AdbScreenrecordProfileResult QueryPhysicalDisplaySize(string adbPath)
    {
        var ensureDevice = EnsureDeviceReady(adbPath);
        if (!ensureDevice.Success)
        {
            return new AdbScreenrecordProfileResult(false, ensureDevice.Message, null);
        }

        var result = RunAdb(adbPath, "shell wm size");
        if (!result.Success)
        {
            return new AdbScreenrecordProfileResult(false, string.IsNullOrWhiteSpace(result.Message) ? "Failed to query display size" : result.Message, null);
        }

        var match = Regex.Match(result.Output ?? string.Empty, @"(\d+)\s*x\s*(\d+)");
        if (!match.Success ||
            !int.TryParse(match.Groups[1].Value, out var width) ||
            !int.TryParse(match.Groups[2].Value, out var height) ||
            width <= 0 ||
            height <= 0)
        {
            return new AdbScreenrecordProfileResult(false, "Could not parse 'adb shell wm size' output", null);
        }

        var profile = AdbScreenrecordProfile.Create(width, height);
        return new AdbScreenrecordProfileResult(true, string.Empty, profile);
    }

    public static ProcessStartInfo BuildShellCaptureStartInfo(string adbPath, AdbScreenrecordProfile profile)
    {
        return new ProcessStartInfo
        {
            FileName = adbPath,
            Arguments =
                $"exec-out screenrecord --output-format=h264 --size {profile.CaptureWidth}x{profile.CaptureHeight} --bit-rate {profile.BitrateBps} --time-limit 180 -",
            UseShellExecute = false,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            CreateNoWindow = true,
        };
    }

    public static CommandResult RunAdb(string adbPath, string arguments)
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

    public readonly record struct CommandResult(bool Success, string Message, string Output);
}

internal readonly record struct AdbTunnelResult(bool Success, string Message);

internal readonly record struct AdbScreenrecordProfileResult(bool Success, string Message, AdbScreenrecordProfile? Profile);

internal readonly record struct AdbScreenrecordProfile(
    int DisplayWidth,
    int DisplayHeight,
    int CaptureWidth,
    int CaptureHeight,
    int BitrateBps,
    int TargetFps)
{
    public static AdbScreenrecordProfile Create(int displayWidth, int displayHeight)
    {
        const int maxLongEdge = 640;
        const int targetFps = 60;
        var longEdge = Math.Max(displayWidth, displayHeight);
        var scale = Math.Min(1d, maxLongEdge / (double)longEdge);
        var captureWidth = AlignTo16((int)Math.Round(displayWidth * scale));
        var captureHeight = AlignTo16((int)Math.Round(displayHeight * scale));
        var pixelBudget = captureWidth * captureHeight;
        var bitrateBps = Math.Clamp(pixelBudget * 4, 600_000, 1_600_000);
        return new AdbScreenrecordProfile(
            displayWidth,
            displayHeight,
            captureWidth,
            captureHeight,
            bitrateBps,
            targetFps);
    }

    public SessionConfig ToSessionConfig()
    {
        return new SessionConfig(
            Codec: "video/avc",
            Preset: "ADB_SHELL_CAPTURE",
            AdaptationMode: "ADB_SHELL_CAPTURE",
            Transport: "ADB_EXEC_OUT_SCREENRECORD_H264",
            Width: CaptureWidth,
            Height: CaptureHeight,
            Fps: TargetFps,
            Bitrate: BitrateBps,
            StreamMode: "single",
            EnhancementEnabled: false,
            EnhancementCodec: null,
            EnhancementMaxWidth: 0,
            EnhancementMaxHeight: 0,
            RoiMode: "none");
    }

    private static int AlignTo16(int value)
    {
        var clamped = Math.Max(16, value);
        return Math.Max(16, (clamped / 16) * 16);
    }
}
