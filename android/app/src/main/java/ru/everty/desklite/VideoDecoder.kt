package ru.everty.desklite

import android.media.MediaCodec
import android.media.MediaCodecList
import android.media.MediaFormat
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
    private var codec: MediaCodec? = null
    val isReady: Boolean get() = codec != null

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
                    PerfStats.frameDecoded()
                    if (PerfStats.nativeTotalFrames() == 1L) Log.i(TAG, "First frame rendered to surface ($mime)")
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

    companion object {
        private const val TAG = "EvdVideoDecoder"
        private val decoders = ConcurrentHashMap<String, VideoDecoder>()
        @Volatile private var currentSurface: Surface? = null

        @Volatile var onDimensionsAvailable: ((Int, Int) -> Unit)? = null

        // Cache codec support per mime type — checked once, never rechecked.
        // ConcurrentHashMap: isMimeSupported() is called from Rust JNI background threads.
        private val mimeSupported = ConcurrentHashMap<String, Boolean>()

        private fun isMimeSupported(mime: String): Boolean =
            mimeSupported.getOrPut(mime) {
                try {
                    val ok = MediaCodecList(MediaCodecList.REGULAR_CODECS).codecInfos.any { info ->
                        !info.isEncoder && info.supportedTypes.any { it.equals(mime, ignoreCase = true) }
                    }
                    Log.i(TAG, "Device codec check: $mime → supported=$ok")
                    ok
                } catch (e: Exception) {
                    Log.e(TAG, "isMimeSupported($mime) error: $e")
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
         */
        @JvmStatic
        fun isDecodeSupported(mime: String): Boolean = isMimeSupported(mime)

        @JvmStatic
        fun setSurface(surface: Surface?) {
            Log.i(TAG, "setSurface: ${if (surface != null) "surface SET" else "surface CLEARED"}")
            decoders.values.forEach { it.release() }
            decoders.clear()
            currentSurface = surface
            PerfStats.reset()
            // Pre-warm H264 decoder so the first IDR renders instantly.
            // Host always sends H264 regardless of UI codec selection.
            if (surface != null && isMimeSupported("video/avc")) {
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
            // Skip unsupported codecs immediately — don't attempt MediaCodec.configure()
            // and don't disturb any existing (pre-warmed) decoder on the Surface.
            // Example: host sends H265 session config first, then switches to H264.
            // On Unisoc T7250 H265 is not supported — we skip it silently and keep
            // the pre-warmed H264 decoder intact so it's ready when H264 IDR arrives.
            if (!isMimeSupported(mime)) {
                if (isKeyframe) Log.w(TAG, "Codec $mime not supported, skipping — keeping existing decoders")
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
            decoders.values.forEach { it.release() }
            decoders.clear()
            currentSurface = null
            onDimensionsAvailable = null
            PerfStats.reset()
        }
    }
}
