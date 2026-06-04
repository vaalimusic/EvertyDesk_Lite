package com.everty.evertygame.stream

data class StreamMetrics(
    val fps: Int = 0,
    val bitrateKbps: Int = 0,
    val pipelineLatencyMs: Int = 0,
    val framesSent: Long = 0,
    val packetsSent: Long = 0,
    val droppedFrames: Long = 0,
    val resolutionLabel: String = "-",
)

enum class StreamPhase(val label: String) {
    IDLE("Idle"),
    REQUESTING_PERMISSION("Permission"),
    STARTING("Starting"),
    STREAMING("Streaming"),
    ERROR("Error"),
}

data class StreamUiState(
    val phase: StreamPhase = StreamPhase.IDLE,
    val status: String = "Ready to start",
    val activeEndpoint: String? = null,
    val activePreset: QualityPreset? = null,
    val activeCodec: VideoCodec? = null,
    val metrics: StreamMetrics = StreamMetrics(),
    val lastError: String? = null,
) {
    val isBusy: Boolean
        get() = phase == StreamPhase.REQUESTING_PERMISSION ||
            phase == StreamPhase.STARTING ||
            phase == StreamPhase.STREAMING
}
