using System.Text;

namespace ReceiverNative;

internal static class ReceiverTrace
{
    private static readonly object Sync = new();
    private static bool _initialized;

    public static string LogFilePath { get; } = Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
        "EvertyNativeReceiver",
        "receiver-native.log");

    public static void Initialize()
    {
        lock (Sync)
        {
            if (_initialized)
            {
                return;
            }

            Directory.CreateDirectory(Path.GetDirectoryName(LogFilePath)!);
            AppendLine("=== Receiver session started ===");
            _initialized = true;
        }
    }

    public static void Log(string message)
    {
        try
        {
            Initialize();
            AppendLine(message);
        }
        catch
        {
        }
    }

    public static void Log(Exception exception, string context)
    {
        try
        {
            Initialize();
            AppendLine($"{context}: {exception}");
        }
        catch
        {
        }
    }

    private static void AppendLine(string message)
    {
        var line = $"[{DateTime.Now:yyyy-MM-dd HH:mm:ss.fff}] {message}{Environment.NewLine}";
        lock (Sync)
        {
            File.AppendAllText(LogFilePath, line, Encoding.UTF8);
        }
    }
}
