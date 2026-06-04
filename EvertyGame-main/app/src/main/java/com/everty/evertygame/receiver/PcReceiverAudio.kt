package com.everty.evertygame.receiver

import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioManager
import android.media.AudioTrack
import android.os.Build
import org.json.JSONObject
import java.nio.charset.StandardCharsets
import kotlin.math.max

internal data class PcReceiverAudioConfig(
    val codec: String,
    val sampleRate: Int,
    val channels: Int,
    val bytesPerSample: Int,
) {
    companion object {
        fun parse(payload: ByteArray): PcReceiverAudioConfig? {
            return runCatching {
                val json = JSONObject(String(payload, StandardCharsets.UTF_8))
                val sampleRate = json.optInt("sampleRate", 0)
                val channels = json.optInt("channels", 0)
                val bytesPerSample = json.optInt("bytesPerSample", 0)
                if (sampleRate <= 0 || channels <= 0 || bytesPerSample <= 0) {
                    null
                } else {
                    PcReceiverAudioConfig(
                        codec = json.optString("codec", "pcm_s16le"),
                        sampleRate = sampleRate,
                        channels = channels,
                        bytesPerSample = bytesPerSample,
                    )
                }
            }.getOrNull()
        }
    }
}

internal class PcReceiverAudioFrameReassembler(
    private val onFrameReady: (ByteArray) -> Unit,
) {
    private val frames = LinkedHashMap<Int, AudioFrameAssembly>()
    private var latestFrameIdSeen = -1

    fun reset() {
        frames.clear()
        latestFrameIdSeen = -1
    }

    fun onPacket(packet: PcReceiverClientController.EvrtPacket) {
        if (packet.packetCount <= 0 || packet.packetIndex >= packet.packetCount) {
            return
        }

        if (packet.frameId > latestFrameIdSeen) {
            val iterator = frames.keys.iterator()
            while (iterator.hasNext()) {
                val key = iterator.next()
                if (key < packet.frameId) {
                    iterator.remove()
                }
            }
            latestFrameIdSeen = packet.frameId
        }

        val assembly = frames.getOrPut(packet.frameId) { AudioFrameAssembly(packet.packetCount) }
        if (assembly.packetCount != packet.packetCount || assembly.isSet(packet.packetIndex)) {
            return
        }

        assembly.set(packet.packetIndex, packet.payload)
        if (!assembly.isComplete) {
            return
        }

        frames.remove(packet.frameId)
        onFrameReady(assembly.join())
    }

    private class AudioFrameAssembly(
        val packetCount: Int,
    ) {
        private val parts = arrayOfNulls<ByteArray>(packetCount)
        private var received = 0

        val isComplete: Boolean
            get() = received == packetCount

        fun isSet(index: Int): Boolean = parts[index] != null

        fun set(index: Int, payload: ByteArray) {
            parts[index] = payload
            received += 1
        }

        fun join(): ByteArray {
            val totalSize = parts.sumOf { it?.size ?: 0 }
            val joined = ByteArray(totalSize)
            var offset = 0
            for (part in parts) {
                val bytes = part ?: continue
                System.arraycopy(bytes, 0, joined, offset, bytes.size)
                offset += bytes.size
            }
            return joined
        }
    }
}

internal class PcReceiverAudioPlaybackSink {
    private val sync = Any()
    private var audioTrack: AudioTrack? = null
    private var config: PcReceiverAudioConfig? = null
    private var lowLatencyMode = false

    fun applyConfig(config: PcReceiverAudioConfig, lowLatencyMode: Boolean) {
        if (!config.codec.equals("pcm_s16le", ignoreCase = true) || config.bytesPerSample != 2) {
            throw IllegalArgumentException("Unsupported audio format: ${config.codec}")
        }

        val channels = if (config.channels > 1) 2 else 1
        val channelMask = if (channels == 1) AudioFormat.CHANNEL_OUT_MONO else AudioFormat.CHANNEL_OUT_STEREO
        val minBufferSize = AudioTrack.getMinBufferSize(config.sampleRate, channelMask, AudioFormat.ENCODING_PCM_16BIT)
        if (minBufferSize <= 0) {
            throw IllegalStateException("AudioTrack min buffer unavailable")
        }

        val targetBufferSize = max(
            minBufferSize,
            config.sampleRate * channels * config.bytesPerSample / if (lowLatencyMode) 40 else 16,
        )
        val audioAttributes = AudioAttributes.Builder()
            .setUsage(AudioAttributes.USAGE_MEDIA)
            .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC)
            .build()
        val audioFormat = AudioFormat.Builder()
            .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
            .setSampleRate(config.sampleRate)
            .setChannelMask(channelMask)
            .build()

        val builder = AudioTrack.Builder()
            .setAudioAttributes(audioAttributes)
            .setAudioFormat(audioFormat)
            .setBufferSizeInBytes(targetBufferSize)
            .setTransferMode(AudioTrack.MODE_STREAM)
            .setSessionId(AudioManager.AUDIO_SESSION_ID_GENERATE)

        if (Build.VERSION.SDK_INT >= 26) {
            builder.setPerformanceMode(AudioTrack.PERFORMANCE_MODE_LOW_LATENCY)
        }

        val track = builder.build()
        if (track.state != AudioTrack.STATE_INITIALIZED) {
            track.release()
            throw IllegalStateException("AudioTrack failed to initialize")
        }

        track.play()
        synchronized(sync) {
            releaseLocked()
            this.config = config.copy(channels = channels)
            this.lowLatencyMode = lowLatencyMode
            audioTrack = track
        }
    }

    fun enqueuePcmFrame(payload: ByteArray) {
        if (payload.isEmpty()) {
            return
        }

        val (track, config, lowLatencyMode) = synchronized(sync) {
            Triple(audioTrack, config, this.lowLatencyMode)
        }
        track ?: return
        config ?: return
        val written = track.write(payload, 0, payload.size, AudioTrack.WRITE_NON_BLOCKING)
        if (!lowLatencyMode || written >= payload.size) {
            return
        }

        // Prioritize current audio over backlog to reduce A/V hitching on weaker Android boxes.
        runCatching {
            track.pause()
            track.flush()
            track.play()
        }

        val bytesPerMs = max(1, (config.sampleRate * config.channels * config.bytesPerSample) / 1000)
        val tailBytes = minOf(payload.size - written.coerceAtLeast(0), bytesPerMs * 20)
        if (tailBytes <= 0) {
            return
        }
        val offset = payload.size - tailBytes
        track.write(payload, offset, tailBytes, AudioTrack.WRITE_NON_BLOCKING)
    }

    fun reset() {
        synchronized(sync) {
            audioTrack?.pause()
            audioTrack?.flush()
            audioTrack?.play()
        }
    }

    fun release() {
        synchronized(sync) {
            releaseLocked()
            config = null
            lowLatencyMode = false
        }
    }

    private fun releaseLocked() {
        runCatching { audioTrack?.pause() }
        runCatching { audioTrack?.flush() }
        runCatching { audioTrack?.stop() }
        runCatching { audioTrack?.release() }
        audioTrack = null
    }
}
