package com.everty.receiver.transport

object TransportProtocol {
    const val MAGIC = 0x45565254
    const val VERSION: Byte = 2

    const val TYPE_SESSION_CONFIG: Byte = 1
    const val TYPE_CODEC_CONFIG: Byte = 2
    const val TYPE_VIDEO_FRAME: Byte = 3
    const val TYPE_CONTROL: Byte = 4
    const val TYPE_AUDIO_CONFIG: Byte = 5
    const val TYPE_AUDIO_FRAME: Byte = 6

    const val FLAG_KEYFRAME: Int = 1

    const val HEADER_SIZE = 24
    const val MAX_PACKET_SIZE = 1200
}
