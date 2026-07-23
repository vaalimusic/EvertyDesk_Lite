package com.everty.evertygame.stream

data class AudioStreamConfig(
    val codec: String = "pcm_s16le",
    val sampleRate: Int,
    val channels: Int,
    val bytesPerSample: Int,
) {
    fun toPayload(): ByteArray {
        return buildString {
            append("{")
            append("\"codec\":\"$codec\",")
            append("\"sampleRate\":$sampleRate,")
            append("\"channels\":$channels,")
            append("\"bytesPerSample\":$bytesPerSample")
            append("}")
        }.toByteArray(Charsets.UTF_8)
    }
}
