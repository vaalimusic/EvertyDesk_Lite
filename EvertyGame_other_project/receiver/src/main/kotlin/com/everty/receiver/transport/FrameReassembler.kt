package com.everty.receiver.transport

import java.io.ByteArrayOutputStream
import java.util.LinkedHashMap

class FrameReassembler(
    private val onSessionConfig: (SessionConfig) -> Unit,
    private val onKeyFrameReady: (ByteArray) -> Unit,
    private val onInterFrameReady: (ByteArray) -> Unit,
) {
    private val frames = LinkedHashMap<Int, FrameAssembly>()
    private var latestCodecConfig: ByteArray? = null
    private var latestFrameIdSeen = -1
    private var latestCompletedFrameId = -1
    private var waitingForKeyFrameAfterLoss = false

    var droppedFrames: Long = 0
        private set

    fun onPacket(packet: UdpPacket) {
        when (packet.type) {
            TransportProtocol.TYPE_SESSION_CONFIG -> {
                SessionConfig.parse(packet.payload)?.let { config ->
                    resetRealtimeState()
                    onSessionConfig(config)
                }
            }

            TransportProtocol.TYPE_CODEC_CONFIG -> {
                latestCodecConfig = packet.payload.copyOf()
            }

            TransportProtocol.TYPE_VIDEO_FRAME -> handleVideoPacket(packet)
        }
    }

    private fun handleVideoPacket(packet: UdpPacket) {
        if (packet.packetCount <= 0 || packet.packetIndex >= packet.packetCount) {
            droppedFrames += 1
            return
        }

        if (packet.frameId <= latestCompletedFrameId) {
            droppedFrames += 1
            return
        }

        if (packet.frameId < latestFrameIdSeen) {
            droppedFrames += 1
            return
        }

        if (waitingForKeyFrameAfterLoss && !packet.isKeyFrame) {
            droppedFrames += 1
            return
        }

        if (packet.frameId > latestFrameIdSeen) {
            val droppedIncomplete = dropOlderFramesThan(packet.frameId)
            latestFrameIdSeen = packet.frameId
            if (droppedIncomplete && !packet.isKeyFrame) {
                waitingForKeyFrameAfterLoss = true
                droppedFrames += 1
                return
            }
        }

        if (packet.isKeyFrame) {
            waitingForKeyFrameAfterLoss = false
            dropOlderFramesThan(packet.frameId)
        }

        val assembly = frames.getOrPut(packet.frameId) {
            FrameAssembly(
                frameId = packet.frameId,
                packetCount = packet.packetCount,
                presentationTimeUs = packet.presentationTimeUs,
                isKeyFrame = packet.isKeyFrame,
            )
        }

        if (assembly.packetCount != packet.packetCount || assembly.packetIndexSet(packet.packetIndex)) {
            return
        }

        assembly.parts[packet.packetIndex] = packet.payload
        assembly.received += 1

        if (assembly.received == assembly.packetCount) {
            frames.remove(packet.frameId)
            latestCompletedFrameId = assembly.frameId
            val frameBytes = assembly.join()
            if (assembly.isKeyFrame) {
                waitingForKeyFrameAfterLoss = false
                val codecConfig = latestCodecConfig
                if (codecConfig != null) {
                    onKeyFrameReady(codecConfig + frameBytes)
                } else {
                    droppedFrames += 1
                    waitingForKeyFrameAfterLoss = true
                }
            } else if (!waitingForKeyFrameAfterLoss) {
                onInterFrameReady(frameBytes)
            } else {
                droppedFrames += 1
            }
        }
    }

    private fun dropOlderFramesThan(frameId: Int): Boolean {
        var droppedIncomplete = false
        val iterator = frames.entries.iterator()
        while (iterator.hasNext()) {
            val entry = iterator.next()
            if (entry.key < frameId) {
                iterator.remove()
                droppedFrames += 1
                droppedIncomplete = true
            }
        }
        return droppedIncomplete
    }

    private fun resetRealtimeState() {
        frames.clear()
        latestFrameIdSeen = -1
        latestCompletedFrameId = -1
        waitingForKeyFrameAfterLoss = false
    }

    private data class FrameAssembly(
        val frameId: Int,
        val packetCount: Int,
        val presentationTimeUs: Long,
        val isKeyFrame: Boolean,
        val parts: Array<ByteArray?> = arrayOfNulls(packetCount),
        var received: Int = 0,
    ) {
        fun packetIndexSet(index: Int): Boolean = parts[index] != null

        fun join(): ByteArray {
            val output = ByteArrayOutputStream()
            parts.forEach { part ->
                if (part != null) {
                    output.write(part)
                }
            }
            return output.toByteArray()
        }
    }
}
