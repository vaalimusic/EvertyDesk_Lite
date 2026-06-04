namespace ReceiverNative;

internal enum WindowsSenderPreset
{
    Media,
    Game,
}

internal readonly record struct WindowsSenderPresetSpec(
    string UiLabel,
    string ProtocolPreset,
    int TargetWidth,
    int TargetHeight,
    int TargetFps,
    int TargetBitrateBps,
    int KeyFrameIntervalSeconds);

internal static class WindowsSenderPresetExtensions
{
    public static string ToUiLabel(this WindowsSenderPreset preset) => preset.ToSpec().UiLabel;

    public static WindowsSenderPresetSpec ToSpec(this WindowsSenderPreset preset)
    {
        return preset switch
        {
            WindowsSenderPreset.Media => new WindowsSenderPresetSpec(
                UiLabel: "Media",
                ProtocolPreset: "MEDIA",
                TargetWidth: 1920,
                TargetHeight: 1080,
                TargetFps: 60,
                TargetBitrateBps: 16_500_000,
                KeyFrameIntervalSeconds: 2),
            WindowsSenderPreset.Game => new WindowsSenderPresetSpec(
                UiLabel: "Game",
                ProtocolPreset: "GAME",
                TargetWidth: 1280,
                TargetHeight: 720,
                TargetFps: 60,
                TargetBitrateBps: 8_500_000,
                KeyFrameIntervalSeconds: 1),
            _ => throw new ArgumentOutOfRangeException(nameof(preset), preset, null),
        };
    }
}
