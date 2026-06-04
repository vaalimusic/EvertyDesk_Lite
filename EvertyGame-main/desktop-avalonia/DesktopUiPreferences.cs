using System;
using System.IO;
using System.Text.Json;

namespace Everty.Desktop.Avalonia;

internal sealed class DesktopUiPreferences
{
    private static readonly JsonSerializerOptions JsonOptions = new()
    {
        WriteIndented = true,
    };

    public bool AdvancedVisible { get; set; }

    public bool DiagnosticsVisible { get; set; }

    public string HostPreset { get; set; } = string.Empty;

    public string ClientPreset { get; set; } = string.Empty;

    public string HostCodec { get; set; } = string.Empty;

    public string HostEncoder { get; set; } = string.Empty;

    public string CaptureTarget { get; set; } = string.Empty;

    public string SelectedHostCode { get; set; } = string.Empty;

    public string ControlPlaneUrl { get; set; } = string.Empty;

    public int SelectedTabIndex { get; set; } = 0;

    public bool HostAdaptiveStreamingEnabled { get; set; } = true;

    public int? HostMediaWidth { get; set; }

    public int? HostMediaHeight { get; set; }

    public int? HostMediaFps { get; set; }

    public int? HostMediaBitrateBps { get; set; }

    public int? HostGameWidth { get; set; }

    public int? HostGameHeight { get; set; }

    public int? HostGameFps { get; set; }

    public int? HostGameBitrateBps { get; set; }

    public static string FilePath =>
        Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
            "EvertyDesktopAvalonia",
            "ui-preferences.json");

    public static DesktopUiPreferences Load()
    {
        try
        {
            var filePath = FilePath;
            if (!File.Exists(filePath))
            {
                return new DesktopUiPreferences();
            }

            var json = File.ReadAllText(filePath);
            return JsonSerializer.Deserialize<DesktopUiPreferences>(json, JsonOptions) ?? new DesktopUiPreferences();
        }
        catch
        {
            return new DesktopUiPreferences();
        }
    }

    public void Save()
    {
        try
        {
            var filePath = FilePath;
            Directory.CreateDirectory(Path.GetDirectoryName(filePath)!);
            File.WriteAllText(filePath, JsonSerializer.Serialize(this, JsonOptions));
        }
        catch
        {
            // Ignore preference write failures.
        }
    }
}
