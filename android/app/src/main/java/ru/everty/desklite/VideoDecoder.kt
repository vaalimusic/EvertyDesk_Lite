package ru.everty.desklite

import android.media.MediaCodec
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

    init {
        try {
            val c = MediaCodec.createDecoderByType(mime)

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
                }

                override fun onOutputFormatChanged(mc: MediaCodec, format: MediaFormat) {
                    val w = format.getInteger(MediaFormat.KEY_WIDTH, 0)
                    val h = format.getInteger(MediaFormat.KEY_HEIGHT, 0)
                    if (w > 0 && h > 0) onDimensionsAvailable?.invoke(w, h)
                }

                override fun onError(mc: MediaCodec, e: MediaCodec.CodecException) {
                    Log.e(TAG, "Codec error ($mime): $e  transient=${e.isTransient}  recoverable=${e.isRecoverable}")
                    hasError = !e.isTransient
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

        @JvmStatic
        fun setSurface(surface: Surface?) {
            decoders.values.forEach { it.release() }
            decoders.clear()
            currentSurface = surface
            PerfStats.reset()
        }

        @JvmStatic
        fun decodeFrame(codec: String, data: ByteArray, isKeyframe: Boolean,
                        width: Int = 0, height: Int = 0): Boolean {
            PerfStats.bytesReceived(data.size)
            if (isKeyframe) PerfStats.idrReceived()
            val surface = currentSurface ?: return false
            val mime = when (codec.uppercase()) {
                "H264"         -> "video/avc"
                "H265", "HEVC" -> "video/hevc"
                "AV1"          -> "video/av01"
                else           -> return false
            }
            val dec = decoders.computeIfAbsent(codec) {
                val w = if (width > 0) width else 1920
                val h = if (height > 0) height else 1080
                VideoDecoder(mime, surface, w, h)
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
