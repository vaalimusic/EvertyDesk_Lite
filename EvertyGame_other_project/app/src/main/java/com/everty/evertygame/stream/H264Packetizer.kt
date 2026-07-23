package com.everty.evertygame.stream

import java.nio.ByteBuffer
import java.nio.ByteOrder
import kotlin.math.ceil

class H264Packetizer(
    private val maxPacketSize: Int = TransportProtocol.MAX_PACKET_SIZE,
) {
    private val maxPayloadSize = maxPacketSize - TransportProtocol.HEADER_SIZE

    fun buildSessionConfigPacket(payload: ByteArray): ByteArray {
        require(payload.size <= maxPayloadSize) { "Session config payload is too large for a single datagram" }
        return buildPacket(
            type = TransportProtocol.TYPE_SESSION_CONFIG,
            flags = 0,
            frameId = 0,
            packetIndex = 0,
            packetCount = 1,
            presentationTimeUs = 0L,
            payload = payload,
        )
    }

    fun buildCodecConfigPacket(payload: ByteArray): ByteArray {
        require(payload.size <= maxPayloadSize) { "Codec config payload is too large for a single datagram" }
        return buildPacket(
            type = TransportProtocol.TYPE_CODEC_CONFIG,
            flags = 0,
            frameId = 0,
            packetIndex = 0,
            packetCount = 1,
            presentationTimeUs = 0L,
            payload = payload,
        )
    }

    fun buildEnhancementConfigPacket(payload: ByteArray): ByteArray {
        require(payload.size <= maxPayloadSize) { "Enhancement config payload is too large for a single datagram" }
        return buildPacket(
            type = TransportProtocol.TYPE_ENHANCEMENT_CONFIG,
            flags = 0,
            frameId = 0,
            packetIndex = 0,
            packetCount = 1,
            presentationTimeUs = 0L,
            payload = payload,
        )
    }

    fun buildRoiMetadataPacket(
        frameId: Int,
        presentationTimeUs: Long,
        payload: ByteArray,
    ): ByteArray {
        require(payload.size <= maxPayloadSize) { "ROI metadata payload is too large for a single datagram" }
        return buildPacket(
            type = TransportProtocol.TYPE_ROI_METADATA,
            flags = 0,
            frameId = frameId,
            packetIndex = 0,
            packetCount = 1,
            presentationTimeUs = presentationTimeUs,
            payload = payload,
        )
    }

    fun buildControlPacket(payload: ByteArray): ByteArray {
        require(payload.size <= maxPayloadSize) { "Control payload is too large for a single datagram" }
        return buildPacket(
            type = TransportProtocol.TYPE_CONTROL,
            flags = 0,
            frameId = 0,
            packetIndex = 0,
            packetCount = 1,
            presentationTimeUs = 0L,
            payload = payload,
        )
    }

    fun buildAudioConfigPacket(payload: ByteArray): ByteArray {
        require(payload.size <= maxPayloadSize) { "Audio config payload is too large for a single datagram" }
        return buildPacket(
            type = TransportProtocol.TYPE_AUDIO_CONFIG,
            flags = 0,
            frameId = 0,
            packetIndex = 0,
            packetCount = 1,
            presentationTimeUs = 0L,
            payload = payload,
        )
    }

    fun packetizeAudioFrame(
        frameId: Int,
        presentationTimeUs: Long,
        payload: ByteArray,
    ): List<ByteArray> {
        require(payload.isNotEmpty()) { "Audio payload must not be empty" }
        val packetCount = ceil(payload.size / maxPayloadSize.toDouble()).toInt().coerceAtLeast(1)
        val packets = ArrayList<ByteArray>(packetCount)

        for (packetIndex in 0 until packetCount) {
            val start = packetIndex * maxPayloadSize
            val end = minOf(start + maxPayloadSize, payload.size)
            val chunk = payload.copyOfRange(start, end)
            packets += buildPacket(
                type = TransportProtocol.TYPE_AUDIO_FRAME,
                flags = 0,
                frameId = frameId,
                packetIndex = packetIndex,
                packetCount = packetCount,
                presentationTimeUs = presentationTimeUs,
                payload = chunk,
            )
        }

        return packets
    }

    fun packetizeVideoFrame(
        frameId: Int,
        presentationTimeUs: Long,
        isKeyFrame: Boolean,
        payload: ByteArray,
    ): List<ByteArray> {
        require(payload.isNotEmpty()) { "Video payload must not be empty" }
        val packetCount = ceil(payload.size / maxPayloadSize.toDouble()).toInt().coerceAtLeast(1)
        val flags = if (isKeyFrame) TransportProtocol.FLAG_KEYFRAME else 0
        val packets = ArrayList<ByteArray>(packetCount)

        for (packetIndex in 0 until packetCount) {
            val start = packetIndex * maxPayloadSize
            val end = minOf(start + maxPayloadSize, payload.size)
            val chunk = payload.copyOfRange(start, end)
            packets += buildPacket(
                type = TransportProtocol.TYPE_VIDEO_FRAME,
                flags = flags,
                frameId = frameId,
                packetIndex = packetIndex,
                packetCount = packetCount,
                presentationTimeUs = presentationTimeUs,
                payload = chunk,
            )
        }

        return packets
    }

    fun packetizeEnhancementFrame(
        frameId: Int,
        presentationTimeUs: Long,
        isKeyFrame: Boolean,
        payload: ByteArray,
    ): List<ByteArray> {
        require(payload.isNotEmpty()) { "Enhancement payload must not be empty" }
        val packetCount = ceil(payload.size / maxPayloadSize.toDouble()).toInt().coerceAtLeast(1)
        val flags = if (isKeyFrame) TransportProtocol.FLAG_KEYFRAME else 0
        val packets = ArrayList<ByteArray>(packetCount)

        for (packetIndex in 0 until packetCount) {
            val start = packetIndex * maxPayloadSize
            val end = minOf(start + maxPayloadSize, payload.size)
            val chunk = payload.copyOfRange(start, end)
            packets += buildPacket(
                type = TransportProtocol.TYPE_ENHANCEMENT_FRAME,
                flags = flags,
                frameId = frameId,
                packetIndex = packetIndex,
                packetCount = packetCount,
                presentationTimeUs = presentationTimeUs,
                payload = chunk,
            )
        }

        return packets
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
        val buffer = ByteBuffer.allocate(TransportProtocol.HEADER_SIZE + payload.size)
            .order(ByteOrder.BIG_ENDIAN)

        buffer.putInt(TransportProtocol.MAGIC)
        buffer.put(TransportProtocol.VERSION)
        buffer.put(type)
        buffer.putShort(flags.toShort())
        buffer.putInt(frameId)
        buffer.putShort(packetIndex.toShort())
        buffer.putShort(packetCount.toShort())
        buffer.putLong(presentationTimeUs)
        buffer.put(payload)

        return buffer.array()
    }
}
