package com.everty.evertygame.stream

sealed interface ControlMessage {
    data object RequestKeyFrame : ControlMessage

    data class ReceiverFeedback(
        val pressure: PressureLevel,
        val backlogFrames: Int,
        val queueDrops: Long,
        val decodeFps: Int,
        val assemblyDelayMs: Int,
        val arrivalDeltaMs: Int,
        val decodeDeltaMs: Int,
        val presentDeltaMs: Int,
    ) : ControlMessage

    enum class PressureLevel {
        NORMAL,
        HIGH,
        CRITICAL,
    }

    companion object {
        private val kindRegex = Regex("\"kind\"\\s*:\\s*\"([^\"]+)\"")
        private val pressureRegex = Regex("\"pressure\"\\s*:\\s*\"([^\"]+)\"")
        private val backlogRegex = Regex("\"backlogFrames\"\\s*:\\s*(\\d+)")
        private val queueDropsRegex = Regex("\"queueDrops\"\\s*:\\s*(\\d+)")
        private val decodeFpsRegex = Regex("\"decodeFps\"\\s*:\\s*(\\d+)")
        private val assemblyDelayRegex = Regex("\"assemblyDelayMs\"\\s*:\\s*(\\d+)")
        private val arrivalDeltaRegex = Regex("\"arrivalDeltaMs\"\\s*:\\s*(-?\\d+)")
        private val decodeDeltaRegex = Regex("\"decodeDeltaMs\"\\s*:\\s*(-?\\d+)")
        private val presentDeltaRegex = Regex("\"presentDeltaMs\"\\s*:\\s*(-?\\d+)")

        fun parse(payload: ByteArray): ControlMessage? {
            val raw = payload.toString(Charsets.UTF_8)
            return when (kindRegex.find(raw)?.groupValues?.getOrNull(1)) {
                "request_keyframe" -> RequestKeyFrame
                "receiver_feedback" -> {
                    val pressure = when (pressureRegex.find(raw)?.groupValues?.getOrNull(1)) {
                        "critical" -> PressureLevel.CRITICAL
                        "high" -> PressureLevel.HIGH
                        else -> PressureLevel.NORMAL
                    }
                    ReceiverFeedback(
                        pressure = pressure,
                        backlogFrames = backlogRegex.find(raw)?.groupValues?.getOrNull(1)?.toIntOrNull() ?: 0,
                        queueDrops = queueDropsRegex.find(raw)?.groupValues?.getOrNull(1)?.toLongOrNull() ?: 0L,
                        decodeFps = decodeFpsRegex.find(raw)?.groupValues?.getOrNull(1)?.toIntOrNull() ?: 0,
                        assemblyDelayMs = assemblyDelayRegex.find(raw)?.groupValues?.getOrNull(1)?.toIntOrNull() ?: 0,
                        arrivalDeltaMs = arrivalDeltaRegex.find(raw)?.groupValues?.getOrNull(1)?.toIntOrNull() ?: -1,
                        decodeDeltaMs = decodeDeltaRegex.find(raw)?.groupValues?.getOrNull(1)?.toIntOrNull() ?: -1,
                        presentDeltaMs = presentDeltaRegex.find(raw)?.groupValues?.getOrNull(1)?.toIntOrNull() ?: -1,
                    )
                }

                else -> null
            }
        }
    }
}
