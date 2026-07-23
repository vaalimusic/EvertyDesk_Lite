package com.everty.receiver.decoder

import java.io.IOException
import java.io.InputStream
import java.util.ArrayDeque

data class DecoderQueueStats(
    val queuedUnits: Int = 0,
    val queuedBytes: Int = 0,
    val droppedUnits: Long = 0,
    val waitingForKeyFrame: Boolean = false,
)

class BlockingByteQueueInputStream(
    private val maxQueuedUnits: Int = 2,
    private val maxQueuedBytes: Int = 96 * 1024,
    private val onStatsChanged: (DecoderQueueStats) -> Unit = {},
) : InputStream() {
    private data class QueuedUnit(
        val bytes: ByteArray,
        val isKeyFrame: Boolean,
    )

    private val lock = Object()
    private val chunks = ArrayDeque<QueuedUnit>()
    private var closed = false
    private var waitingForKeyFrame = false
    private var droppedUnits = 0L
    private var queuedBytes = 0
    private var current: ByteArray? = null
    private var currentOffset = 0

    fun enqueue(bytes: ByteArray, isKeyFrame: Boolean) {
        val snapshot: DecoderQueueStats
        synchronized(lock) {
            if (closed) {
                return
            }

            when {
                waitingForKeyFrame && !isKeyFrame -> {
                    droppedUnits += 1
                }

                waitingForKeyFrame && isKeyFrame -> {
                    chunks.clear()
                    queuedBytes = 0
                    waitingForKeyFrame = false
                    chunks.addLast(QueuedUnit(bytes = bytes, isKeyFrame = true))
                    queuedBytes += bytes.size
                    lock.notifyAll()
                }

                chunks.size >= maxQueuedUnits || queuedBytes + bytes.size > maxQueuedBytes -> {
                    droppedUnits += chunks.size.toLong()
                    chunks.clear()
                    queuedBytes = 0

                    if (isKeyFrame) {
                        chunks.addLast(QueuedUnit(bytes = bytes, isKeyFrame = true))
                        queuedBytes += bytes.size
                        lock.notifyAll()
                    } else {
                        droppedUnits += 1
                        waitingForKeyFrame = true
                    }
                }

                else -> {
                    chunks.addLast(QueuedUnit(bytes = bytes, isKeyFrame = isKeyFrame))
                    queuedBytes += bytes.size
                    lock.notifyAll()
                }
            }

            snapshot = snapshotLocked()
        }

        onStatsChanged(snapshot)
    }

    fun waitForKeyFrame() {
        val snapshot: DecoderQueueStats
        synchronized(lock) {
            if (closed) {
                return
            }
            chunks.clear()
            queuedBytes = 0
            current = null
            currentOffset = 0
            waitingForKeyFrame = true
            snapshot = snapshotLocked()
            lock.notifyAll()
        }
        onStatsChanged(snapshot)
    }

    override fun read(): Int {
        val single = ByteArray(1)
        val count = read(single, 0, 1)
        return if (count == -1) -1 else single[0].toInt() and 0xFF
    }

    override fun read(buffer: ByteArray, offset: Int, length: Int): Int {
        synchronized(lock) {
            while (true) {
                ensureCurrentChunk()
                val chunk = current
                if (chunk != null) {
                    val remaining = chunk.size - currentOffset
                    val toCopy = minOf(length, remaining)
                    chunk.copyInto(
                        destination = buffer,
                        destinationOffset = offset,
                        startIndex = currentOffset,
                        endIndex = currentOffset + toCopy,
                    )
                    currentOffset += toCopy
                    if (currentOffset >= chunk.size) {
                        current = null
                        currentOffset = 0
                    }
                    return toCopy
                }

                if (closed) {
                    return -1
                }

                try {
                    lock.wait()
                } catch (e: InterruptedException) {
                    Thread.currentThread().interrupt()
                    throw IOException("Interrupted while waiting for H264 stream", e)
                }
            }
        }
    }

    override fun close() {
        val snapshot: DecoderQueueStats
        synchronized(lock) {
            closed = true
            chunks.clear()
            queuedBytes = 0
            current = null
            currentOffset = 0
            snapshot = snapshotLocked()
            lock.notifyAll()
        }
        onStatsChanged(snapshot)
    }

    private fun ensureCurrentChunk() {
        if (current == null && chunks.isNotEmpty()) {
            val next = chunks.removeFirst()
            current = next.bytes
            currentOffset = 0
            queuedBytes -= next.bytes.size
        }
    }

    private fun snapshotLocked(): DecoderQueueStats {
        val currentBytesRemaining = current?.let { it.size - currentOffset } ?: 0
        return DecoderQueueStats(
            queuedUnits = chunks.size + if (current != null) 1 else 0,
            queuedBytes = queuedBytes + currentBytesRemaining,
            droppedUnits = droppedUnits,
            waitingForKeyFrame = waitingForKeyFrame,
        )
    }
}
