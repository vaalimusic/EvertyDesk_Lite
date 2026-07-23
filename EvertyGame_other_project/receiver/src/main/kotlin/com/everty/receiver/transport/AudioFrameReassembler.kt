package com.everty.receiver.transport

import java.io.ByteArrayOutputStream
import java.util.LinkedHashMap

class AudioFrameReassembler(
    private val onAudioFrameReady: (ByteArray) -> Unit,
) {
    private val staleFrameWindow = 24
    private val frames = LinkedHashMap<Int, AudioFrameAssembly>()

    var droppedFrames: Long = 0
        private set

    fun onPacket(packet: UdpPacket) {
        if (packet.packetCount <= 0 || packet.packetIndex >= packet.packetCount) {
            droppedFrames += 1
            return
        }

        pruneOldFrames(packet.frameId)

        val assembly = frames.getOrPut(packet.frameId) {
            AudioFrameAssembly(
                frameId = packet.frameId,
                packetCount = packet.packetCount,
            )
        }

        if (assembly.packetCount != packet.packetCount || assembly.parts[packet.packetIndex] != null) {
            return
        }

        assembly.parts[packet.packetIndex] = packet.payload
        assembly.received += 1
        if (assembly.received == assembly.packetCount) {
            frames.remove(packet.frameId)
            onAudioFrameReady(assembly.join())
        }
    }

    private fun pruneOldFrames(currentFrameId: Int) {
        val iterator = frames.entries.iterator()
        while (iterator.hasNext()) {
            val entry = iterator.next()
            if (entry.key < currentFrameId - staleFrameWindow) {
                iterator.remove()
                droppedFrames += 1
            }
        }
    }

    private data class AudioFrameAssembly(
        val frameId: Int,
        val packetCount: Int,
        val parts: Array<ByteArray?> = arrayOfNulls(packetCount),
        var received: Int = 0,
    ) {
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
