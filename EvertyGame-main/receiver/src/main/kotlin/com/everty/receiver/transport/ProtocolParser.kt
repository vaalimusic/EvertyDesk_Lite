package com.everty.receiver.transport

import java.nio.ByteBuffer
import java.nio.ByteOrder

object ProtocolParser {
    fun parse(datagram: ByteArray, length: Int): UdpPacket? {
        if (length < TransportProtocol.HEADER_SIZE) {
            return null
        }

        val buffer = ByteBuffer.wrap(datagram, 0, length).order(ByteOrder.BIG_ENDIAN)
        val magic = buffer.int
        if (magic != TransportProtocol.MAGIC) {
            return null
        }

        val version = buffer.get()
        if (version != 1.toByte() && version != TransportProtocol.VERSION) {
            return null
        }

        val type = buffer.get()
        val flags = buffer.short.toInt() and 0xFFFF
        val frameId = buffer.int
        val packetIndex = buffer.short.toInt() and 0xFFFF
        val packetCount = buffer.short.toInt() and 0xFFFF
        val presentationTimeUs = buffer.long
        val payload = ByteArray(length - TransportProtocol.HEADER_SIZE)
        buffer.get(payload)

        return UdpPacket(
            type = type,
            flags = flags,
            frameId = frameId,
            packetIndex = packetIndex,
            packetCount = packetCount,
            presentationTimeUs = presentationTimeUs,
            payload = payload,
        )
    }
}
