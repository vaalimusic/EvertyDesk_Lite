package com.everty.receiver.transport

sealed interface ControlMessage {
    fun toPayload(): ByteArray

    data object RequestKeyFrame : ControlMessage {
        override fun toPayload(): ByteArray = """{"kind":"request_keyframe"}""".toByteArray(Charsets.UTF_8)
    }

    data class ReceiverFeedback(
        val pressure: PressureLevel,
        val backlogFrames: Int,
        val queueDrops: Long,
        val decodeFps: Int,
    ) : ControlMessage {
        override fun toPayload(): ByteArray {
            val pressureLabel = when (pressure) {
                PressureLevel.NORMAL -> "normal"
                PressureLevel.HIGH -> "high"
            }
            return buildString {
                append("{")
                append("\"kind\":\"receiver_feedback\",")
                append("\"pressure\":\"$pressureLabel\",")
                append("\"backlogFrames\":$backlogFrames,")
                append("\"queueDrops\":$queueDrops,")
                append("\"decodeFps\":$decodeFps")
                append("}")
            }.toByteArray(Charsets.UTF_8)
        }
    }

    enum class PressureLevel {
        NORMAL,
        HIGH,
    }
}
