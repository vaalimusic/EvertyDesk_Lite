package ru.everty.desklite

import android.media.MediaCodec
import android.media.MediaCodecList
import android.media.MediaFormat
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.util.Log
import android.view.Surface
import java.util.concurrent.ConcurrentHashMap

/**
 * Hardware video decoder — async MediaCodec.Callback + Surface output.
 *
 * Async mode: frames rendered in onOutputBufferAvailable immediately upon decode,
 * with no polling. Eliminates up to 43ms dequeueOutputBuffer wait from sync mode.
 *
 * Input buffering: codec offers free input slots via onInputBufferAvailable.
 * If a frame arrives when no slot is available it is queued (max 1 pending frame —
 * older frames are dropped to protect against latency buildup).
 */
class VideoDecoder private constructor(private val mime: String, surface: Surface,
                                      initWidth: Int = 1920, initHeight: Int = 1080) {

    private data class Frame(val data: ByteArray, val isKeyframe: Boolean)

    // Protected by inputLock — both codec thread and JNI thread access these
    private val inputLock = Any()
    private val freeInputSlots = ArrayDeque<Int>()   // codec → us: available input buffer indices
    private val pendingFrames  = ArrayDeque<Frame>() // us → codec: frames waiting for a slot

    @Volatile private var hasError = false
    // Volatile: the watchdog (main-thread Handler) can null this via
    // release() concurrently with enqueue()/submitFrame() running on a JNI
    // (Rust) thread. A torn read here would just mean one frame's
    // MediaCodec calls throw (caught in submitFrame's existing try/catch,
    // degrading to a dropped frame) — not a crash — but volatile at least
    // makes the null visible promptly instead of an arbitrarily stale read.
    @Volatile private var codec: MediaCodec? = null
    val isReady: Boolean get() = codec != null

    // ROADMAP.md task #30 — live-found on real hardware (MI_8/Adreno630):
    // under sustained real-time HEVC load, this SoC's hardware decoder can
    // silently wedge — it keeps ACCEPTING input (queueInputBuffer succeeds,
    // no MediaCodec.Callback.onError fires) but permanently stops calling
    // onOutputBufferAvailable, so the picture freezes on its last rendered
    // frame or shows nothing at all, indefinitely, with every health signal
    // this codebase already tracks (enqueue() returning true, decoded_fps
    // staying up on the HOST side, since that only reflects successful
    // submission — see enqueue()'s own doc) reporting perfectly healthy.
    // Confirmed live: `onOutputBufferAvailable`'s own periodic frame-count
    // log never advanced past its first checkpoint while IDR frames kept
    // arriving and being accepted. The watchdog below is the standard
    // mitigation for exactly this class of hardware/driver decoder wedge —
    // it doesn't try to fix the wedge, it detects "data flowing in, nothing
    // flowing out" and forces a full decoder recreation, letting the next
    // periodic host keyframe (`KEYFRAME_INTERVAL`, evrt2_experiment.rs)
    // resync the picture instead of the stream staying frozen forever.
    @Volatile private var lastInputAtMs = SystemClock.elapsedRealtime()
    @Volatile private var lastOutputAtMs = SystemClock.elapsedRealtime()
    @Volatile private var everRendered = false

    init {
        var c: MediaCodec? = null
        try {
            c = MediaCodec.createDecoderByType(mime)

            c.setCallback(object : MediaCodec.Callback() {
                override fun onInputBufferAvailable(mc: MediaCodec, index: Int) {
                    val frame = synchronized(inputLock) {
                        pendingFrames.removeFirstOrNull()
                            ?: run { freeInputSlots.addLast(index); null }
                    } ?: return
                    submitFrame(mc, index, frame)
                }

                override fun onOutputBufferAvailable(mc: MediaCodec, index: Int, info: MediaCodec.BufferInfo) {
                    mc.releaseOutputBuffer(index, true) // GPU render — zero copy!
                    lastOutputAtMs = SystemClock.elapsedRealtime()
                    everRendered = true
                    PerfStats.frameDecoded()
                    val total = PerfStats.nativeTotalFrames()
                    if (total == 1L) Log.i(TAG, "First frame rendered to surface ($mime)")
                    // ROADMAP.md task #30 diagnostic: a live black-screen-
                    // after-a-few-seconds report needs to distinguish "the
                    // decoder stopped OUTPUTTING frames" (this counter
                    // stalling) from "input keeps being accepted but
                    // rendering silently died" — `enqueue()`'s own success
                    // only proves submission, never actual render (see its
                    // doc comment), so this is the one place that can tell
                    // the two apart.
                    if (total % 60 == 0L) Log.i(TAG, "onOutputBufferAvailable: total=$total ($mime)")
                }

                override fun onOutputFormatChanged(mc: MediaCodec, format: MediaFormat) {
                    val w = format.getInteger(MediaFormat.KEY_WIDTH, 0)
                    val h = format.getInteger(MediaFormat.KEY_HEIGHT, 0)
                    Log.i(TAG, "onOutputFormatChanged: ${w}x${h}  dimCallback=${onDimensionsAvailable != null}")
                    if (w > 0 && h > 0) onDimensionsAvailable?.invoke(w, h)
                }

                override fun onError(mc: MediaCodec, e: MediaCodec.CodecException) {
                    Log.e(TAG, "Codec error ($mime): $e  transient=${e.isTransient}  recoverable=${e.isRecoverable}")
                    if (!e.isTransient) {
                        hasError = true
                        // Remove from decoders so next IDR triggers a fresh decoder,
                        // not the permanently broken one (isReady stays true while codec != null).
                        decoders.values.removeIf { it === this@VideoDecoder }
                    }
                }
            })

            val format = MediaFormat.createVideoFormat(mime, initWidth, initHeight)
            format.setInteger(MediaFormat.KEY_MAX_INPUT_SIZE, 4 * 1024 * 1024)
            if (android.os.Build.VERSION.SDK_INT >= 30) {
                format.setInteger(MediaFormat.KEY_LOW_LATENCY, 1)
            } else {
                // Vendor extension supported by Qualcomm/MediaTek on API < 30
                format.setInteger("low-latency", 1)
            }
            // 120fps operating rate: signals hardware to maintain higher clock for 30fps content
            format.setInteger(MediaFormat.KEY_OPERATING_RATE, 120)
            format.setInteger(MediaFormat.KEY_PRIORITY, 0)
            c.configure(format, surface, null, 0)
            c.start()
            codec = c
            Log.i(TAG, "Decoder started (async, low-latency): $mime")
        } catch (e: Exception) {
            Log.e(TAG, "Failed to start decoder for $mime: $e")
            // Release codec so it doesn't hold the Surface, blocking other decoders
            try { c?.release() } catch (_: Exception) {}
        }
    }

    private fun submitFrame(mc: MediaCodec, index: Int, frame: Frame) {
        try {
            val buf = mc.getInputBuffer(index) ?: return
            buf.clear()
            val len = minOf(frame.data.size, buf.remaining())
            buf.put(frame.data, 0, len)
            val flags = if (frame.isKeyframe) MediaCodec.BUFFER_FLAG_KEY_FRAME else 0
            mc.queueInputBuffer(index, 0, len, System.nanoTime() / 1_000L, flags)
            PerfStats.frameSubmitted()
        } catch (e: Exception) {
            Log.e(TAG, "Submit error: $e")
        }
    }

    fun enqueue(data: ByteArray, isKeyframe: Boolean): Boolean {
        val c = codec ?: return false
        if (hasError) return false
        lastInputAtMs = SystemClock.elapsedRealtime()

        val frame = Frame(data, isKeyframe)
        val slot = synchronized(inputLock) {
            freeInputSlots.removeFirstOrNull().also { idx ->
                if (idx == null) {
                    // Codec busy — queue frame, drop overflow to avoid latency buildup
                    if (pendingFrames.size > 1) pendingFrames.removeFirst()
                    pendingFrames.addLast(frame)
                }
            }
        }
        slot?.let { submitFrame(c, it, frame) }
        return true
    }

    fun release() {
        val c = codec ?: return
        codec = null
        try { c.stop() } catch (_: Exception) {}
        try { c.release() } catch (_: Exception) {}
        synchronized(inputLock) {
            freeInputSlots.clear()
            pendingFrames.clear()
        }
    }

    /** See the watchdog doc above `lastInputAtMs` for why this exists. */
    private fun isStalled(nowMs: Long): Boolean =
        everRendered &&
            (nowMs - lastInputAtMs) < STALL_INPUT_GRACE_MS &&
            (nowMs - lastOutputAtMs) > STALL_TIMEOUT_MS

    companion object {
        private const val TAG = "EvdVideoDecoder"
        private val decoders = ConcurrentHashMap<String, VideoDecoder>()
        @Volatile private var currentSurface: Surface? = null

        // ROADMAP.md task #30 watchdog thresholds — see the doc comment on
        // `lastInputAtMs` for the live hardware-decoder-wedge finding this
        // exists to recover from. `STALL_INPUT_GRACE_MS` gates the check on
        // "is data actually still arriving" (a decoder that's idle because
        // the STREAM stopped, not because IT stalled, must never be
        // reset — that's not a bug, that's just no traffic). Host's own
        // `KEYFRAME_INTERVAL` is 2s, so `STALL_TIMEOUT_MS` sits comfortably
        // below that: a stall is detected and the decoder recreated in time
        // for the SAME upcoming periodic keyframe to resync it, rather than
        // waiting for the one after.
        private const val STALL_TIMEOUT_MS = 1500L
        private const val STALL_INPUT_GRACE_MS = 1000L
        private const val WATCHDOG_INTERVAL_MS = 500L
        private val watchdogHandler = Handler(Looper.getMainLooper())
        @Volatile private var watchdogRunning = false

        private val watchdogRunnable = object : Runnable {
            override fun run() {
                val now = SystemClock.elapsedRealtime()
                for ((codecKey, dec) in decoders.entries.toList()) {
                    if (dec.isStalled(now)) {
                        Log.w(TAG, "Decoder STALLED (no output for ${now - dec.lastOutputAtMs}ms while input kept arriving) — recreating: $codecKey")
                        decoders.remove(codecKey, dec)
                        dec.release()
                    }
                }
                watchdogHandler.postDelayed(this, WATCHDOG_INTERVAL_MS)
            }
        }

        private fun ensureWatchdogRunning() {
            if (!watchdogRunning) {
                watchdogRunning = true
                watchdogHandler.postDelayed(watchdogRunnable, WATCHDOG_INTERVAL_MS)
            }
        }

        private fun stopWatchdog() {
            watchdogRunning = false
            watchdogHandler.removeCallbacks(watchdogRunnable)
        }

        @Volatile var onDimensionsAvailable: ((Int, Int) -> Unit)? = null

        // HW-aware capability cache — the question is "is there a REAL hardware
        // decoder", not "can this mime be decoded at all". The distinction
        // matters for AV1: many phones ship Google's software dav1d decoder
        // (c2.android.av1-dav1d), which passes a plain mime check but cannot
        // sustain real-time 60fps game streaming (observed: 8 frames in,
        // 0 frames out, black screen).
        // ConcurrentHashMap: queried from Rust JNI background threads.
        private val mimeHwSupported = ConcurrentHashMap<String, Boolean>()

        private fun isHardwareMimeSupported(mime: String): Boolean =
            mimeHwSupported.getOrPut(mime) {
                try {
                    val ok = MediaCodecList(MediaCodecList.REGULAR_CODECS).codecInfos.any { info ->
                        if (info.isEncoder) return@any false
                        if (!info.supportedTypes.any { it.equals(mime, ignoreCase = true) }) return@any false
                        if (android.os.Build.VERSION.SDK_INT >= 29) {
                            info.isHardwareAccelerated
                        } else {
                            // Pre-Android-10 heuristic: software decoders are the
                            // Google-provided components. Everything else is vendor HW.
                            val n = info.name
                            !n.startsWith("OMX.google.") && !n.startsWith("c2.android.")
                        }
                    }
                    Log.i(TAG, "Device HW codec check: $mime → hardware=$ok")
                    ok
                } catch (e: Exception) {
                    Log.e(TAG, "isHardwareMimeSupported($mime) error: $e")
                    false
                }
            }

        /**
         * Public entry point for Rust (JNI) to query real device HW decode
         * capability before advertising codec support to the host — see
         * `Java_ru_everty_desklite_NativeClient_nativeIsDecodeSupported` in
         * android_ffi.rs. Without this, codec capability was hardcoded false
         * for H265/AV1 on Android regardless of actual device support,
         * silently downgrading every session to H264 no matter what the user
         * picked in the game-mode codec selector.
         *
         * HARDWARE-only on purpose: what we advertise to the host must mean
         * "this device can decode a real-time 60fps stream of this codec".
         * A software decoder passing the plain mime check does not qualify —
         * advertising it produces a connected session with a black screen
         * (the exact AV1/dav1d failure this check exists to prevent).
         */
        @JvmStatic
        fun isDecodeSupported(mime: String): Boolean = isHardwareMimeSupported(mime)

        @JvmStatic
        fun setSurface(surface: Surface?) {
            Log.i(TAG, "setSurface: ${if (surface != null) "surface SET" else "surface CLEARED"}")
            decoders.values.forEach { it.release() }
            decoders.clear()
            currentSurface = surface
            PerfStats.reset()
            if (surface != null) ensureWatchdogRunning() else stopWatchdog()
            // Pre-warm H264 decoder so the first IDR renders instantly.
            // Host always sends H264 regardless of UI codec selection.
            if (surface != null && isHardwareMimeSupported("video/avc")) {
                val h264 = VideoDecoder("video/avc", surface, 1920, 1080)
                if (h264.isReady) {
                    decoders["H264"] = h264
                    Log.i(TAG, "H264 decoder pre-warmed and ready")
                }
            }
        }

        @JvmStatic
        fun decodeFrame(codec: String, data: ByteArray, isKeyframe: Boolean,
                        width: Int = 0, height: Int = 0): Boolean {
            PerfStats.bytesReceived(data.size)
            if (isKeyframe) {
                PerfStats.idrReceived()
                Log.i(TAG, "IDR frame: codec=$codec ${width}x${height} surface=${currentSurface != null}")
            }
            val surface = currentSurface ?: run {
                Log.w(TAG, "decodeFrame: surface is null, dropping $codec frame")
                return false
            }
            val mime = when (codec.uppercase()) {
                "H264"         -> "video/avc"
                "H265", "HEVC" -> "video/hevc"
                "AV1"          -> "video/av01"
                else           -> return false
            }
            // Skip codecs without a REAL hardware decoder immediately — don't
            // attempt MediaCodec.configure() and don't disturb any existing
            // (pre-warmed) decoder on the Surface.
            // Two failure classes this guards against:
            //  • Unisoc T7250: no H265 decoder at all → configure() would fail.
            //  • dav1d software AV1 (c2.android.av1-dav1d): configure() SUCCEEDS,
            //    evicts the pre-warmed H264 decoder from the Surface, then decodes
            //    nothing (8 frames in, 0 out) → permanent black screen. Software
            //    decoders must never claim the Surface in a real-time session.
            if (!isHardwareMimeSupported(mime)) {
                if (isKeyframe) Log.w(TAG, "Codec $mime has no hardware decoder, skipping — keeping existing decoders")
                return false
            }
            // Release decoders for OTHER supported codecs before creating a new one.
            // A Surface can only have one MediaCodec producer at a time.
            if (!decoders.containsKey(codec) && decoders.isNotEmpty()) {
                Log.i(TAG, "Codec change to $codec: releasing ${decoders.size} old decoder(s)")
                val old = decoders.values.toList()
                decoders.clear()
                old.forEach { it.release() }
            }

            val dec = decoders.computeIfAbsent(codec) {
                val w = if (width > 0) width else 1920
                val h = if (height > 0) height else 1080
                VideoDecoder(mime, surface, w, h)
            }
            if (!dec.isReady) {
                // Decoder failed to start — remove so next IDR retries
                decoders.remove(codec, dec)
                return false
            }
            return dec.enqueue(data, isKeyframe)
        }

        @JvmStatic
        fun releaseAll() {
            stopWatchdog()
            decoders.values.forEach { it.release() }
            decoders.clear()
            currentSurface = null
            onDimensionsAvailable = null
            PerfStats.reset()
        }
    }
}
