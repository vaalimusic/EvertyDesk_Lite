package com.everty.receiver.transport

data class AudioConfig(
    val codec: String,
    val sampleRate: Int,
    val channels: Int,
    val bytesPerSample: Int,
) {
    companion object {
        private val codecRegex = Regex("\"codec\"\\s*:\\s*\"([^\"]+)\"")
        private val sampleRateRegex = Regex("\"sampleRate\"\\s*:\\s*(\\d+)")
        private val channelsRegex = Regex("\"channels\"\\s*:\\s*(\\d+)")
        private val bytesPerSampleRegex = Regex("\"bytesPerSample\"\\s*:\\s*(\\d+)")

        fun parse(payload: ByteArray): AudioConfig? {
            val raw = payload.toString(Charsets.UTF_8)
            return AudioConfig(
                codec = codecRegex.find(raw)?.groupValues?.getOrNull(1) ?: return null,
                sampleRate = sampleRateRegex.find(raw)?.groupValues?.getOrNull(1)?.toIntOrNull() ?: return null,
                channels = channelsRegex.find(raw)?.groupValues?.getOrNull(1)?.toIntOrNull() ?: return null,
                bytesPerSample = bytesPerSampleRegex.find(raw)?.groupValues?.getOrNull(1)?.toIntOrNull() ?: return null,
            )
        }
    }
}
