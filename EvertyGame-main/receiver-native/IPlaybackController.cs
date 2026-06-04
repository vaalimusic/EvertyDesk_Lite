namespace ReceiverNative;

internal interface IPlaybackController : IDisposable
{
    event Action<string>? StatusChanged;
    event Action<PlaybackStreamStats>? StreamStatsChanged;
    event Action<PlaybackStreamStats>? EnhancementStreamStatsChanged;
    event Action<long>? FrameDecoded;
    event Action<long>? FramePresented;

    string BackendLabel { get; }
    long LastPresentedBasePresentationTimeUs { get; }

    void UpdateHardwareDecodeMode(HardwareDecodeMode mode);
    void UpdateAggressiveMode(bool enabled);
    void UpdateUltraLowLatencyMode(bool enabled);
    void UpdateAdaptiveJitterBuffer(TimeSpan delay);
    void UpdatePacingWindow(TimeSpan minDelay, TimeSpan maxDelay);
    void ApplySessionConfig(SessionConfig config);
    void EnqueueAccessUnit(byte[] bytes, bool isKeyFrame, long presentationTimeUs);
    void EnqueueEnhancementAccessUnit(byte[] bytes, bool isKeyFrame, long presentationTimeUs, RoiMetadata? metadata);
    void PrepareForSessionStop();
    void PrepareForKeyFrameRecovery();
    void WaitForKeyFrame();
    void ResetEnhancementPath();
}
