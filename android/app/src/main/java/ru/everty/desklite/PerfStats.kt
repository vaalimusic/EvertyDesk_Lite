package ru.everty.desklite

import java.util.concurrent.atomic.AtomicLong

object PerfStats {
    private val lock = Any()
    private val frameTimes = ArrayDeque<Long>()   // ms timestamps of decoded frames (2s window)
    private val submitTimes = ArrayDeque<Long>()  // ms timestamps of codec submissions (FIFO → decode latency)
    private var bitrateBytes = 0L
    private var bitrateWindowStart = System.currentTimeMillis()
    @Volatile var rxKbps = 0
    @Volatile var avgDecodeMs = 0L
    private var sessionStart = System.currentTimeMillis()
    private var idrCount = 0
    // Monotonic total decoded frame counter — read by Rust via JNI for accurate fps_decoded.
    private val totalFrames = AtomicLong(0L)

    fun reset() {
        synchronized(lock) {
            frameTimes.clear()
            submitTimes.clear()
            bitrateBytes = 0
            bitrateWindowStart = System.currentTimeMillis()
            sessionStart = System.currentTimeMillis()
            idrCount = 0
        }
        totalFrames.set(0L)
        rxKbps = 0
        avgDecodeMs = 0L
    }

    fun frameSubmitted() {
        synchronized(lock) { submitTimes.addLast(System.currentTimeMillis()) }
    }

    fun frameDecoded() {
        totalFrames.incrementAndGet()
        val now = System.currentTimeMillis()
        synchronized(lock) {
            frameTimes.addLast(now)
            // Evict frames older than 2 seconds for rolling FPS
            val cutoff = now - 2000L
            while (frameTimes.isNotEmpty() && frameTimes.first() < cutoff) frameTimes.removeFirst()
            // FIFO: match oldest submit to this output for decode latency
            val submitMs = if (submitTimes.isNotEmpty()) submitTimes.removeFirst() else null
            if (submitMs != null) {
                val latency = now - submitMs
                avgDecodeMs = (avgDecodeMs * 7 + latency) / 8  // exponential moving avg
            }
        }
    }

    fun bytesReceived(n: Int) {
        synchronized(lock) {
            bitrateBytes += n.toLong()
            val now = System.currentTimeMillis()
            val elapsed = now - bitrateWindowStart
            if (elapsed >= 1000L) {
                rxKbps = ((bitrateBytes * 8L) / elapsed).toInt()
                bitrateBytes = 0
                bitrateWindowStart = now
            }
        }
    }

    fun idrReceived() {
        synchronized(lock) { idrCount++ }
    }

    val fps: Float get() = synchronized(lock) {
        if (frameTimes.size < 2) return 0f
        val window = (frameTimes.last() - frameTimes.first()).coerceAtLeast(1L)
        (frameTimes.size - 1) * 1000f / window
    }

    val jitterMs: Long get() = synchronized(lock) {
        if (frameTimes.size < 3) return 0L
        val intervals = (1 until frameTimes.size).map { frameTimes[it] - frameTimes[it - 1] }
        val avg = intervals.average()
        intervals.map { kotlin.math.abs(it - avg) }.average().toLong()
    }

    val idrPerMin: Int get() = synchronized(lock) {
        val elapsedMin = (System.currentTimeMillis() - sessionStart) / 60_000.0
        if (elapsedMin < 0.01) 0 else (idrCount / elapsedMin).toInt()
    }

    fun summary(): String {
        val f = "%.1f".format(fps)
        val d = avgDecodeMs
        val j = jitterMs
        val k = rxKbps
        return "${f}fps | dec ${d}ms | jit ${j}ms | ${k}kbps"
    }

    // ── JNI access for Rust feedback loop ─────────────────────────────────────
    // Two separate methods: avoids unsafe LongArray → JPrimitiveArray cast in Rust JNI.
    @JvmStatic
    fun nativeTotalFrames(): Long = totalFrames.get()

    @JvmStatic
    fun nativeAvgDecodeMs(): Long = avgDecodeMs
}
