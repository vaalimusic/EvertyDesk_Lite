package com.everty.evertygame.stream

enum class QualityPreset(
    val uiName: String,
    val targetWidth: Int,
    val targetHeight: Int,
    val fps: Int,
    val bitrateBps: Int,
    val keyFrameIntervalSeconds: Int,
    val syncFrameIntervalMs: Int,
) {
    TOURNAMENT_FIGHTER(
        uiName = "Tournament / Fight Game",
        targetWidth = 1280,
        targetHeight = 720,
        fps = 60,
        bitrateBps = 6_500_000,
        keyFrameIntervalSeconds = 8,
        syncFrameIntervalMs = 1_000,
    ),
    WI_FI_GAMING(
        uiName = "Wi-Fi Gaming",
        targetWidth = 960,
        targetHeight = 540,
        fps = 60,
        bitrateBps = 8_500_000,
        keyFrameIntervalSeconds = 8,
        syncFrameIntervalMs = 850,
    ),
    INSTANT_PLAY(
        uiName = "Instant Play",
        targetWidth = 640,
        targetHeight = 360,
        fps = 60,
        bitrateBps = 2_200_000,
        keyFrameIntervalSeconds = 1,
        syncFrameIntervalMs = 333,
    ),
    COMPETITIVE(
        uiName = "Competitive",
        targetWidth = 1280,
        targetHeight = 720,
        fps = 60,
        bitrateBps = 7_500_000,
        keyFrameIntervalSeconds = 8,
        syncFrameIntervalMs = 1_000,
    ),
    BALANCED_LOW_LATENCY(
        uiName = "Balanced Low Latency",
        targetWidth = 1280,
        targetHeight = 720,
        fps = 60,
        bitrateBps = 8_500_000,
        keyFrameIntervalSeconds = 6,
        syncFrameIntervalMs = 1_000,
    ),
    STABLE_COMPATIBILITY(
        uiName = "Stable Compatibility",
        targetWidth = 1280,
        targetHeight = 720,
        fps = 30,
        bitrateBps = 6_000_000,
        keyFrameIntervalSeconds = 1,
        syncFrameIntervalMs = 1_000,
    ),
    HIGH_QUALITY(
        uiName = "High Quality",
        targetWidth = 1920,
        targetHeight = 1080,
        fps = 30,
        bitrateBps = 12_000_000,
        keyFrameIntervalSeconds = 1,
        syncFrameIntervalMs = 1_000,
    ),
    LOW_END_RECEIVER(
        uiName = "Low-end Receiver",
        targetWidth = 960,
        targetHeight = 540,
        fps = 30,
        bitrateBps = 4_000_000,
        keyFrameIntervalSeconds = 1,
        syncFrameIntervalMs = 500,
    );

    val summary: String
        get() = "${targetWidth}x${targetHeight} / $fps FPS / ${"%.1f".format(bitrateBps / 1_000_000.0)} Mbps"
}
