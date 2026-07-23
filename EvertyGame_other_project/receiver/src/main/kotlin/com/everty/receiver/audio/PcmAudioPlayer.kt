package com.everty.receiver.audio

import com.everty.receiver.transport.AudioConfig
import java.io.Closeable
import java.util.ArrayDeque
import javax.sound.sampled.AudioFormat
import javax.sound.sampled.AudioSystem
import javax.sound.sampled.SourceDataLine

data class AudioPlayerStats(
    val queuedChunks: Int = 0,
    val queuedMs: Int = 0,
    val droppedChunks: Long = 0,
)

class PcmAudioPlayer(
    private val onStatus: (String) -> Unit,
    private val onStats: (AudioPlayerStats) -> Unit,
) : Closeable {
    private val lock = Object()
    private val queue = ArrayDeque<ByteArray>()
    private var running = true
    private var currentConfig: AudioConfig? = null
    private var pendingLineReset = false
    private var line: SourceDataLine? = null
    private var queuedBytes = 0
    private var droppedChunks = 0L
    private var maxQueuedBytes = 24 * 1024

    private val thread = Thread({
        while (running) {
            val chunk = synchronized(lock) {
                while (running && (queue.isEmpty() || pendingLineReset || line == null)) {
                    if (pendingLineReset) {
                        reopenLineLocked()
                    }
                    if (!running) {
                        return@Thread
                    }
                    if (queue.isEmpty() || line == null) {
                        lock.wait(250)
                    }
                }
                val next = queue.pollFirst()
                if (next == null) {
                    publishStatsLocked()
                    return@synchronized null
                }
                queuedBytes -= next.size
                publishStatsLocked()
                next
            }
            if (chunk == null) {
                continue
            }

            runCatching {
                line?.write(chunk, 0, chunk.size)
            }.onFailure {
                onStatus("Audio output stalled")
            }
        }
    }, "EvertyPcmAudioPlayer").apply {
        isDaemon = true
        start()
    }

    fun configure(config: AudioConfig) {
        synchronized(lock) {
            if (currentConfig == config && line != null) {
                return
            }
            currentConfig = config
            queue.clear()
            queuedBytes = 0
            maxQueuedBytes = (config.sampleRate * config.channels * config.bytesPerSample / 25).coerceAtLeast(6 * 1024)
            pendingLineReset = true
            publishStatsLocked()
            lock.notifyAll()
        }
    }

    fun enqueue(pcmBytes: ByteArray) {
        synchronized(lock) {
            if (!running || currentConfig == null) {
                return
            }

            if (queuedBytes + pcmBytes.size > maxQueuedBytes) {
                droppedChunks += queue.size.toLong()
                queue.clear()
                queuedBytes = 0
            }

            queue.addLast(pcmBytes)
            queuedBytes += pcmBytes.size
            publishStatsLocked()
            lock.notifyAll()
        }
    }

    override fun close() {
        synchronized(lock) {
            if (!running) {
                return
            }
            running = false
            queue.clear()
            queuedBytes = 0
            closeLineLocked()
            publishStatsLocked()
            lock.notifyAll()
        }
        thread.join(1_000)
    }

    private fun reopenLineLocked() {
        closeLineLocked()
        val config = currentConfig ?: run {
            pendingLineReset = false
            return
        }

        val format = AudioFormat(
            config.sampleRate.toFloat(),
            (config.bytesPerSample * 8).toInt(),
            config.channels,
            true,
            false,
        )
        val dataLine = AudioSystem.getSourceDataLine(format)
        val targetBufferBytes = (config.sampleRate * config.channels * config.bytesPerSample / 40)
            .coerceAtLeast(4 * 1024)
        dataLine.open(format, targetBufferBytes)
        dataLine.start()
        line = dataLine
        pendingLineReset = false
        onStatus("Audio active ${config.sampleRate} Hz / ${config.channels} ch")
    }

    private fun closeLineLocked() {
        runCatching { line?.drain() }
        runCatching { line?.stop() }
        runCatching { line?.close() }
        line = null
    }

    private fun publishStatsLocked() {
        val config = currentConfig
        val queuedMs = if (config == null || config.sampleRate <= 0 || config.channels <= 0 || config.bytesPerSample <= 0) {
            0
        } else {
            ((queuedBytes * 1000L) / (config.sampleRate * config.channels * config.bytesPerSample)).toInt()
        }
        onStats(
            AudioPlayerStats(
                queuedChunks = queue.size,
                queuedMs = queuedMs,
                droppedChunks = droppedChunks,
            ),
        )
    }
}
