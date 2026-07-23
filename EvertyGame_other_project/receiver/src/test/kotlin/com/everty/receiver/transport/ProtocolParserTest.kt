package com.everty.receiver.transport

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Test
import java.nio.ByteBuffer
import java.nio.ByteOrder

class ProtocolParserTest {
    @Test
    fun `parser decodes valid video packet`() {
        val datagram = buildPacket(
            type = TransportProtocol.TYPE_VIDEO_FRAME,
            flags = TransportProtocol.FLAG_KEYFRAME,
            frameId = 42,
            packetIndex = 1,
            packetCount = 3,
            presentationTimeUs = 999_888L,
            payload = byteArrayOf(10, 20, 30),
        )

        val packet = ProtocolParser.parse(datagram, datagram.size)
        assertNotNull(packet)
        assertEquals(42, packet!!.frameId)
        assertEquals(1, packet.packetIndex)
        assertEquals(3, packet.packetCount)
        assertEquals(999_888L, packet.presentationTimeUs)
        assertEquals(true, packet.isKeyFrame)
        assertEquals(3, packet.payload.size)
    }

    @Test
    fun `parser rejects invalid magic`() {
        val datagram = ByteArray(TransportProtocol.HEADER_SIZE)
        val packet = ProtocolParser.parse(datagram, datagram.size)
        assertNull(packet)
    }

    private fun buildPacket(
        type: Byte,
        flags: Int,
        frameId: Int,
        packetIndex: Int,
        packetCount: Int,
        presentationTimeUs: Long,
        payload: ByteArray,
    ): ByteArray {
        return ByteBuffer.allocate(TransportProtocol.HEADER_SIZE + payload.size)
            .order(ByteOrder.BIG_ENDIAN)
            .putInt(TransportProtocol.MAGIC)
            .put(TransportProtocol.VERSION)
            .put(type)
            .putShort(flags.toShort())
            .putInt(frameId)
            .putShort(packetIndex.toShort())
            .putShort(packetCount.toShort())
            .putLong(presentationTimeUs)
            .put(payload)
            .array()
    }
}
