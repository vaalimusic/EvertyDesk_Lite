package com.everty.evertygame.stream

object TransportProtocol {
    const val MAGIC = 0x45565254
    const val VERSION: Byte = 3

    const val TYPE_SESSION_CONFIG: Byte = 1
    const val TYPE_CODEC_CONFIG: Byte = 2
    const val TYPE_VIDEO_FRAME: Byte = 3
    const val TYPE_CONTROL: Byte = 4
    const val TYPE_AUDIO_CONFIG: Byte = 5
    const val TYPE_AUDIO_FRAME: Byte = 6
    const val TYPE_ENHANCEMENT_CONFIG: Byte = 7
    const val TYPE_ENHANCEMENT_FRAME: Byte = 8
    const val TYPE_ROI_METADATA: Byte = 9

    const val FLAG_KEYFRAME: Int = 1

    const val HEADER_SIZE = 24
    const val MAX_PACKET_SIZE = 1200
    const val MAX_PAYLOAD_SIZE = MAX_PACKET_SIZE - HEADER_SIZE
}
