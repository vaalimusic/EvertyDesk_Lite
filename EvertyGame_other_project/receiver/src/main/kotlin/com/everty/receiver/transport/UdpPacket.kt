package com.everty.receiver.transport

data class UdpPacket(
    val type: Byte,
    val flags: Int,
    val frameId: Int,
    val packetIndex: Int,
    val packetCount: Int,
    val presentationTimeUs: Long,
    val payload: ByteArray,
) {
    val isKeyFrame: Boolean
        get() = flags and TransportProtocol.FLAG_KEYFRAME != 0
}
