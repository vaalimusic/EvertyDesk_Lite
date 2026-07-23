package com.everty.evertygame.stream

import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioPlaybackCaptureConfiguration
import android.media.AudioRecord
import android.media.projection.MediaProjection
import android.os.Build
import java.io.Closeable
import java.util.concurrent.atomic.AtomicBoolean

class PlaybackAudioStreamer(
    private val mediaProjection: MediaProjection,
    private val packetizer: H264Packetizer,
    private val packetSender: PacketSender,
    private val onError: (String) -> Unit,
) : Closeable {
    private val running = AtomicBoolean(false)
    private var audioRecord: AudioRecord? = null
    private var thread: Thread? = null
    private var frameSequence = 0

    fun start() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) {
            return
        }
        if (!running.compareAndSet(false, true)) {
            return
        }

        val profile = buildCaptureProfile()
        val config = AudioStreamConfig(
            sampleRate = profile.sampleRate,
            channels = profile.channels,
            bytesPerSample = 2,
        )

        val captureConfig = AudioPlaybackCaptureConfiguration.Builder(mediaProjection)
            .addMatchingUsage(AudioAttributes.USAGE_MEDIA)
            .addMatchingUsage(AudioAttributes.USAGE_GAME)
            .addMatchingUsage(AudioAttributes.USAGE_UNKNOWN)
            .build()

        val record = AudioRecord.Builder()
            .setAudioFormat(
                AudioFormat.Builder()
                    .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                    .setSampleRate(profile.sampleRate)
                    .setChannelMask(profile.channelMask)
                    .build(),
            )
            .setBufferSizeInBytes(profile.recordBufferBytes)
            .setAudioPlaybackCaptureConfig(captureConfig)
            .build()

        if (record.state != AudioRecord.STATE_INITIALIZED) {
            record.release()
            running.set(false)
            throw IllegalStateException("AudioRecord failed to initialize")
        }

        audioRecord = record
        packetSender.send(packetizer.buildAudioConfigPacket(config.toPayload()))

        thread = Thread({
            val readBuffer = ByteArray(profile.frameBytes)
            try {
                record.startRecording()
                while (running.get()) {
                    val bytesRead = record.read(readBuffer, 0, readBuffer.size, AudioRecord.READ_BLOCKING)
                    if (bytesRead <= 0) {
                        continue
                    }

                    val payload = if (bytesRead == readBuffer.size) {
                        readBuffer.copyOf()
                    } else {
                        readBuffer.copyOfRange(0, bytesRead)
                    }
                    val packets = packetizer.packetizeAudioFrame(
                        frameId = frameSequence++,
                        presentationTimeUs = System.nanoTime() / 1_000L,
                        payload = payload,
                    )
                    packets.forEach(packetSender::send)
                }
            } catch (t: Throwable) {
                if (running.get()) {
                    onError(t.message ?: "Audio capture failed")
                }
            } finally {
                runCatching { record.stop() }
            }
        }, "EvertyAudioCapture").apply {
            isDaemon = true
            start()
        }
    }

    override fun close() {
        if (!running.compareAndSet(true, false)) {
            return
        }
        runCatching { audioRecord?.stop() }
        runCatching { audioRecord?.release() }
        audioRecord = null
        thread?.interrupt()
        thread?.join(1_000)
        thread = null
    }

    private fun buildCaptureProfile(): CaptureProfile {
        val candidates = listOf(
            CaptureProfile(sampleRate = 48_000, channels = 2, channelMask = AudioFormat.CHANNEL_IN_STEREO),
            CaptureProfile(sampleRate = 44_100, channels = 2, channelMask = AudioFormat.CHANNEL_IN_STEREO),
            CaptureProfile(sampleRate = 48_000, channels = 1, channelMask = AudioFormat.CHANNEL_IN_MONO),
        )

        candidates.forEach { candidate ->
            val minBufferBytes = AudioRecord.getMinBufferSize(
                candidate.sampleRate,
                candidate.channelMask,
                AudioFormat.ENCODING_PCM_16BIT,
            )
            if (minBufferBytes > 0) {
                val bytesPerFrame = candidate.channels * 2
                val frameBytes = candidate.sampleRate / 50 * bytesPerFrame
                return candidate.copy(
                    frameBytes = frameBytes,
                    recordBufferBytes = maxOf(minBufferBytes, frameBytes * 2),
                )
            }
        }

        error("No supported playback audio capture profile")
    }

    private data class CaptureProfile(
        val sampleRate: Int,
        val channels: Int,
        val channelMask: Int,
        val frameBytes: Int = 0,
        val recordBufferBytes: Int = 0,
    )
}
