package com.everty.receiver.transport

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class FrameReassemblerTest {
    @Test
    fun `keyframe emits codec config before access unit`() {
        val sessionConfigs = mutableListOf<SessionConfig>()
        val keyFrames = mutableListOf<ByteArray>()
        val interFrames = mutableListOf<ByteArray>()
        val reassembler = FrameReassembler(
            onSessionConfig = sessionConfigs::add,
            onKeyFrameReady = keyFrames::add,
            onInterFrameReady = interFrames::add,
        )

        reassembler.onPacket(
            packet(
                type = TransportProtocol.TYPE_CODEC_CONFIG,
                payload = byteArrayOf(0, 0, 0, 1, 103),
            ),
        )
        reassembler.onPacket(
            packet(
                type = TransportProtocol.TYPE_VIDEO_FRAME,
                frameId = 7,
                packetCount = 2,
                packetIndex = 0,
                flags = TransportProtocol.FLAG_KEYFRAME,
                payload = byteArrayOf(1, 2),
            ),
        )
        reassembler.onPacket(
            packet(
                type = TransportProtocol.TYPE_VIDEO_FRAME,
                frameId = 7,
                packetCount = 2,
                packetIndex = 1,
                flags = TransportProtocol.FLAG_KEYFRAME,
                payload = byteArrayOf(3, 4),
            ),
        )

        assertEquals(1, keyFrames.size)
        assertArrayEquals(byteArrayOf(0, 0, 0, 1, 103, 1, 2, 3, 4), keyFrames.single())
        assertTrue(interFrames.isEmpty())
        assertTrue(sessionConfigs.isEmpty())
    }

    @Test
    fun `session config is parsed and exposed`() {
        var config: SessionConfig? = null
        val reassembler = FrameReassembler(
            onSessionConfig = { config = it },
            onKeyFrameReady = {},
            onInterFrameReady = {},
        )

        reassembler.onPacket(
            packet(
                type = TransportProtocol.TYPE_SESSION_CONFIG,
                payload = """
                    {"codec":"video/avc","preset":"BALANCED_LOW_LATENCY","width":1280,"height":720,"fps":60,"bitrate":8000000}
                """.trimIndent().toByteArray(),
            ),
        )

        assertEquals("video/avc", config?.codec)
        assertEquals("BALANCED_LOW_LATENCY", config?.preset)
        assertEquals(1280, config?.width)
        assertEquals(720, config?.height)
    }

    @Test
    fun `newer frame abandons older incomplete frame`() {
        val keyFrames = mutableListOf<ByteArray>()
        val interFrames = mutableListOf<ByteArray>()
        val reassembler = FrameReassembler(
            onSessionConfig = {},
            onKeyFrameReady = keyFrames::add,
            onInterFrameReady = interFrames::add,
        )

        reassembler.onPacket(
            packet(
                type = TransportProtocol.TYPE_CODEC_CONFIG,
                payload = byteArrayOf(0, 0, 0, 1, 103),
            ),
        )
        reassembler.onPacket(
            packet(
                type = TransportProtocol.TYPE_VIDEO_FRAME,
                frameId = 10,
                packetCount = 2,
                packetIndex = 0,
                flags = TransportProtocol.FLAG_KEYFRAME,
                payload = byteArrayOf(1, 2),
            ),
        )
        reassembler.onPacket(
            packet(
                type = TransportProtocol.TYPE_VIDEO_FRAME,
                frameId = 11,
                packetCount = 1,
                packetIndex = 0,
                flags = TransportProtocol.FLAG_KEYFRAME,
                payload = byteArrayOf(9, 9),
            ),
        )

        assertEquals(1, keyFrames.size)
        assertArrayEquals(byteArrayOf(0, 0, 0, 1, 103, 9, 9), keyFrames.single())
        assertTrue(interFrames.isEmpty())
        assertEquals(1L, reassembler.droppedFrames)
    }

    @Test
    fun `after incomplete interframe loss receiver waits for next keyframe`() {
        val keyFrames = mutableListOf<ByteArray>()
        val interFrames = mutableListOf<ByteArray>()
        val reassembler = FrameReassembler(
            onSessionConfig = {},
            onKeyFrameReady = keyFrames::add,
            onInterFrameReady = interFrames::add,
        )

        reassembler.onPacket(packet(type = TransportProtocol.TYPE_CODEC_CONFIG, payload = byteArrayOf(0, 0, 0, 1, 103)))
        reassembler.onPacket(
            packet(
                type = TransportProtocol.TYPE_VIDEO_FRAME,
                frameId = 1,
                packetCount = 1,
                packetIndex = 0,
                flags = TransportProtocol.FLAG_KEYFRAME,
                payload = byteArrayOf(1),
            ),
        )
        reassembler.onPacket(
            packet(
                type = TransportProtocol.TYPE_VIDEO_FRAME,
                frameId = 2,
                packetCount = 2,
                packetIndex = 0,
                payload = byteArrayOf(2),
            ),
        )
        reassembler.onPacket(
            packet(
                type = TransportProtocol.TYPE_VIDEO_FRAME,
                frameId = 3,
                packetCount = 1,
                packetIndex = 0,
                payload = byteArrayOf(3),
            ),
        )
        reassembler.onPacket(
            packet(
                type = TransportProtocol.TYPE_VIDEO_FRAME,
                frameId = 4,
                packetCount = 1,
                packetIndex = 0,
                flags = TransportProtocol.FLAG_KEYFRAME,
                payload = byteArrayOf(4),
            ),
        )

        assertEquals(2, keyFrames.size)
        assertTrue(interFrames.isEmpty())
        assertArrayEquals(byteArrayOf(0, 0, 0, 1, 103, 4), keyFrames.last())
        assertTrue(reassembler.droppedFrames >= 2L)
    }

    private fun packet(
        type: Byte,
        payload: ByteArray,
        frameId: Int = 0,
        packetIndex: Int = 0,
        packetCount: Int = 1,
        flags: Int = 0,
    ) = UdpPacket(
        type = type,
        flags = flags,
        frameId = frameId,
        packetIndex = packetIndex,
        packetCount = packetCount,
        presentationTimeUs = 0,
        payload = payload,
    )
}
