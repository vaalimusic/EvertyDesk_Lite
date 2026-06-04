package com.everty.receiver

data class ReceiverSnapshot(
    val listening: Boolean = false,
    val status: String = "Idle",
    val sessionCodec: String = "-",
    val decodePath: String = "-",
    val audioStatus: String = "-",
    val audioQueuedMs: Int = 0,
    val audioDroppedChunks: Long = 0,
    val sessionPreset: String = "-",
    val resolution: String = "-",
    val fpsTarget: Int = 0,
    val bitrateMbps: Double = 0.0,
    val packetsReceived: Long = 0,
    val framesAssembled: Long = 0,
    val framesDropped: Long = 0,
    val framesDecoded: Long = 0,
    val decodeFps: Int = 0,
    val decoderBacklogFrames: Int = 0,
    val decoderBacklogKb: Int = 0,
    val decoderQueueDrops: Long = 0,
    val decoderWaitingForKeyFrame: Boolean = false,
)
