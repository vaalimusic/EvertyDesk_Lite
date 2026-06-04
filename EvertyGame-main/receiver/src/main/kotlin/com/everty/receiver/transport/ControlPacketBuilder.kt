package com.everty.receiver.transport

import java.nio.ByteBuffer
import java.nio.ByteOrder

class ControlPacketBuilder(
    private val maxPacketSize: Int = TransportProtocol.MAX_PACKET_SIZE,
) {
    private val maxPayloadSize = maxPacketSize - TransportProtocol.HEADER_SIZE

    fun build(message: ControlMessage): ByteArray {
        val payload = message.toPayload()
        require(payload.size <= maxPayloadSize) { "Control payload is too large for a single datagram" }

        val buffer = ByteBuffer.allocate(TransportProtocol.HEADER_SIZE + payload.size)
            .order(ByteOrder.BIG_ENDIAN)

        buffer.putInt(TransportProtocol.MAGIC)
        buffer.put(TransportProtocol.VERSION)
        buffer.put(TransportProtocol.TYPE_CONTROL)
        buffer.putShort(0)
        buffer.putInt(0)
        buffer.putShort(0)
        buffer.putShort(1)
        buffer.putLong(0L)
        buffer.put(payload)

        return buffer.array()
    }
}
