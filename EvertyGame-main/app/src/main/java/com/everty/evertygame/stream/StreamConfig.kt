package com.everty.evertygame.stream

data class StreamConfig(
    val host: String,
    val port: Int,
    val transport: StreamTransport,
    val preset: QualityPreset,
    val targetFps: Int,
    val targetBitrateBps: Int,
    val codec: VideoCodec,
    val audioEnabled: Boolean,
    val adaptationMode: AdaptationMode,
    val touchLatencySprintEnabled: Boolean,
    val gamepadBoostEnabled: Boolean,
    val adaptiveRoiSplitStreamEnabled: Boolean,
)
