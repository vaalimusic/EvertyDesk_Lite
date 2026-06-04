package com.everty.receiver.transport

data class SessionConfig(
    val codec: String,
    val preset: String,
    val width: Int,
    val height: Int,
    val fps: Int,
    val bitrate: Int,
) {
    val resolutionLabel: String
        get() = "${width}x${height}"

    companion object {
        private val codecRegex = Regex("\"codec\"\\s*:\\s*\"([^\"]+)\"")
        private val presetRegex = Regex("\"preset\"\\s*:\\s*\"([^\"]+)\"")
        private val widthRegex = Regex("\"width\"\\s*:\\s*(\\d+)")
        private val heightRegex = Regex("\"height\"\\s*:\\s*(\\d+)")
        private val fpsRegex = Regex("\"fps\"\\s*:\\s*(\\d+)")
        private val bitrateRegex = Regex("\"bitrate\"\\s*:\\s*(\\d+)")

        fun parse(payload: ByteArray): SessionConfig? {
            val raw = payload.toString(Charsets.UTF_8)
            return SessionConfig(
                codec = codecRegex.find(raw)?.groupValues?.getOrNull(1) ?: return null,
                preset = presetRegex.find(raw)?.groupValues?.getOrNull(1) ?: return null,
                width = widthRegex.find(raw)?.groupValues?.getOrNull(1)?.toIntOrNull() ?: return null,
                height = heightRegex.find(raw)?.groupValues?.getOrNull(1)?.toIntOrNull() ?: return null,
                fps = fpsRegex.find(raw)?.groupValues?.getOrNull(1)?.toIntOrNull() ?: return null,
                bitrate = bitrateRegex.find(raw)?.groupValues?.getOrNull(1)?.toIntOrNull() ?: return null,
            )
        }
    }
}
