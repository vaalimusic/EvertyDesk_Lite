package com.everty.evertygame.stream

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Test
import java.nio.ByteBuffer
import java.nio.ByteOrder

class H264PacketizerTest {
    private val packetizer = H264Packetizer(maxPacketSize = 40)

    @Test
    fun `single packet preserves header metadata`() {
        val packet = packetizer.packetizeVideoFrame(
            frameId = 77,
            presentationTimeUs = 123_456L,
            isKeyFrame = true,
            payload = byteArrayOf(1, 2, 3, 4),
        ).single()

        val header = ByteBuffer.wrap(packet).order(ByteOrder.BIG_ENDIAN)
        assertEquals(TransportProtocol.MAGIC, header.int)
        assertEquals(TransportProtocol.VERSION, header.get())
        assertEquals(TransportProtocol.TYPE_VIDEO_FRAME, header.get())
        assertEquals(TransportProtocol.FLAG_KEYFRAME, header.short.toInt())
        assertEquals(77, header.int)
        assertEquals(0, header.short.toInt())
        assertEquals(1, header.short.toInt())
        assertEquals(123_456L, header.long)
        assertArrayEquals(byteArrayOf(1, 2, 3, 4), packet.copyOfRange(TransportProtocol.HEADER_SIZE, packet.size))
    }

    @Test
    fun `large frame is fragmented with stable numbering`() {
        val payload = ByteArray(21) { index -> (index + 1).toByte() }
        val packets = packetizer.packetizeVideoFrame(
            frameId = 5,
            presentationTimeUs = 999L,
            isKeyFrame = false,
            payload = payload,
        )

        assertEquals(2, packets.size)

        val firstHeader = ByteBuffer.wrap(packets[0]).order(ByteOrder.BIG_ENDIAN)
        firstHeader.position(8)
        assertEquals(5, firstHeader.int)
        assertEquals(0, firstHeader.short.toInt())
        assertEquals(2, firstHeader.short.toInt())

        val secondHeader = ByteBuffer.wrap(packets[1]).order(ByteOrder.BIG_ENDIAN)
        secondHeader.position(8)
        assertEquals(5, secondHeader.int)
        assertEquals(1, secondHeader.short.toInt())
        assertEquals(2, secondHeader.short.toInt())

        val reassembled = packets.flatMap { packet ->
            packet.copyOfRange(TransportProtocol.HEADER_SIZE, packet.size).asIterable()
        }.toByteArray()

        assertArrayEquals(payload, reassembled)
    }
}
