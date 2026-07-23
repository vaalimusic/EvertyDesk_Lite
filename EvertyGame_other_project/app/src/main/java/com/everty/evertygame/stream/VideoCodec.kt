package com.everty.evertygame.stream

import android.media.MediaCodecInfo
import android.media.MediaCodecList

enum class VideoCodec(
    val uiName: String,
    val mimeType: String,
    val ffmpegFormat: String,
    val summary: String,
) {
    AVC(
        uiName = "H.264 / AVC",
        mimeType = "video/avc",
        ffmpegFormat = "h264",
        summary = "Fastest path for low-latency play and best compatibility.",
    ),
    HEVC(
        uiName = "H.265 / HEVC",
        mimeType = "video/hevc",
        ffmpegFormat = "hevc",
        summary = "Lower bitrate at similar quality. Experimental for realtime.",
    );

    companion object {
        fun supportedEncoders(): List<VideoCodec> {
            val codecInfos = MediaCodecList(MediaCodecList.REGULAR_CODECS).codecInfos
            return entries.filter { codec ->
                codecInfos.any { info ->
                    info.isEncoder &&
                        info.supportedTypes.any { type -> type.equals(codec.mimeType, ignoreCase = true) } &&
                        supportsSurfaceInput(info, codec.mimeType)
                }
            }.ifEmpty {
                listOf(AVC)
            }
        }

        fun fromMimeType(mimeType: String): VideoCodec? =
            entries.firstOrNull { it.mimeType.equals(mimeType, ignoreCase = true) }

        private fun supportsSurfaceInput(
            codecInfo: MediaCodecInfo,
            mimeType: String,
        ): Boolean {
            val capabilities = runCatching {
                codecInfo.getCapabilitiesForType(mimeType)
            }.getOrNull() ?: return false

            return capabilities.colorFormats.any { colorFormat ->
                colorFormat == MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface
            }
        }
    }
}
