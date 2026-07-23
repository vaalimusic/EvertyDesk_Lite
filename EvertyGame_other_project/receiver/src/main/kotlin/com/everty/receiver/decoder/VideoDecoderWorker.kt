package com.everty.receiver.decoder

import org.bytedeco.javacv.FFmpegFrameGrabber
import org.bytedeco.javacv.Frame
import org.bytedeco.javacv.FrameGrabber
import java.io.Closeable
import java.util.concurrent.atomic.AtomicBoolean

class VideoDecoderWorker(
    private val codecMimeType: String,
    private val decoderPreference: DecoderPreference,
    private val ultraRealtime: Boolean,
    private val onFrame: (Frame) -> Unit,
    private val onStatus: (String) -> Unit,
    private val onError: (String) -> Unit,
    private val onQueueStats: (DecoderQueueStats) -> Unit,
    private val onDecodePathChanged: (String) -> Unit,
) : Closeable {
    private val running = AtomicBoolean(false)
    private val inputStream = BlockingByteQueueInputStream(
        maxQueuedUnits = if (ultraRealtime) 1 else 2,
        maxQueuedBytes = if (ultraRealtime) 48 * 1024 else 96 * 1024,
        onStatsChanged = onQueueStats,
    )
    private var thread: Thread? = null

    fun start() {
        if (!running.compareAndSet(false, true)) {
            return
        }

        thread = Thread({
            val attempts = buildAttempts(codecMimeType)
            onStatus("Waiting for ${codecLabel(codecMimeType)} stream")

            attempts.forEachIndexed { index, attempt ->
                if (!running.get()) {
                    return@forEachIndexed
                }

                var grabber: FFmpegFrameGrabber? = null
                try {
                    onDecodePathChanged(attempt.label)
                    grabber = FFmpegFrameGrabber(inputStream, 0).apply {
                        format = ffmpegFormat(codecMimeType)
                        imageMode = FrameGrabber.ImageMode.COLOR
                        setOption("fflags", "nobuffer")
                        setOption("flags", "low_delay")
                        setOption("flags2", "fast")
                        setOption("avioflags", "direct")
                        setOption("flush_packets", "1")
                        setOption("probesize", "32")
                        setOption("analyzeduration", "0")
                        setOption("max_delay", "0")
                        setOption("reorder_queue_size", "0")
                        videoCodecName = attempt.videoCodecName
                        attempt.options.forEach { (key, value) ->
                            setOption(key, value)
                        }
                        attempt.videoOptions.forEach { (key, value) ->
                            setVideoOption(key, value)
                        }
                        start()
                    }
                    onStatus("Decoder active (${attempt.label})")
                    var decodedFramesForAttempt = 0

                    while (running.get()) {
                        val frame: Frame = grabber.grabImage() ?: break
                        if (attempt.requiresDisplayableFrames && !frame.hasDisplayableImage()) {
                            runCatching { frame.close() }
                            error("Decoder ${attempt.label} returned hardware-only frames")
                        }
                        try {
                            onFrame(frame)
                            decodedFramesForAttempt += 1
                        } finally {
                            runCatching { frame.close() }
                        }
                    }

                    if (!running.get()) {
                        return@Thread
                    }
                    if (decodedFramesForAttempt == 0) {
                        error("Decoder ${attempt.label} produced no frames")
                    }
                    return@Thread
                } catch (t: Throwable) {
                    if (!running.get()) {
                        return@Thread
                    }

                    val hasFallback = index < attempts.lastIndex
                    if (hasFallback) {
                        inputStream.waitForKeyFrame()
                        onStatus("Decoder ${attempt.label} failed. Waiting for keyframe before fallback")
                    } else {
                        onError(t.message ?: "Decoder failed")
                    }
                } finally {
                    runCatching { grabber?.stop() }
                    runCatching { grabber?.close() }
                }
            }
        }, "EvertyVideoDecoder").apply {
            isDaemon = true
            start()
        }
    }

    fun offerAccessUnit(bytes: ByteArray, isKeyFrame: Boolean) {
        if (running.get()) {
            inputStream.enqueue(bytes, isKeyFrame)
        }
    }

    override fun close() {
        if (!running.compareAndSet(true, false)) {
            return
        }
        inputStream.close()
        thread?.interrupt()
        thread?.join(1_000)
        thread = null
    }

    private fun buildAttempts(codecMimeType: String): List<DecoderAttempt> {
        val attempts = mutableListOf<DecoderAttempt>()
        val isWindows = System.getProperty("os.name")
            ?.contains("Windows", ignoreCase = true) == true
        val softwareThreads = Runtime.getRuntime().availableProcessors().coerceIn(2, 4).toString()

        fun addD3d11va() {
            attempts += DecoderAttempt(
                label = "D3D11VA",
                options = mapOf(
                    "hwaccel" to "d3d11va",
                    "threads" to "1",
                ),
                requiresDisplayableFrames = true,
            )
        }

        fun addDxva2() {
            attempts += DecoderAttempt(
                label = "DXVA2",
                options = mapOf(
                    "hwaccel" to "dxva2",
                    "threads" to "1",
                ),
                requiresDisplayableFrames = true,
            )
        }

        fun addNvdec() {
            nvidiaDecoderName(codecMimeType)?.let { decoderName ->
                attempts += DecoderAttempt(
                    label = "NVDEC / CUDA",
                    videoCodecName = decoderName,
                    options = mapOf(
                        "hwaccel" to "cuda",
                        "hwaccel_output_format" to "cuda",
                        "threads" to "1",
                    ),
                    videoOptions = mapOf(
                        "surfaces" to "2",
                    ),
                    requiresDisplayableFrames = true,
                )
            }
        }

        fun addSoftware() {
            attempts += DecoderAttempt(
                label = "Software",
                options = mapOf(
                    "threads" to softwareThreads,
                    "thread_type" to "frame",
                    "skip_loop_filter" to "nonref",
                ),
            )
        }

        if (isWindows) {
            when (decoderPreference) {
                DecoderPreference.AUTO -> {
                    addD3d11va()
                    addDxva2()
                    addNvdec()
                    addSoftware()
                }

                DecoderPreference.D3D11VA -> {
                    addD3d11va()
                    addSoftware()
                }

                DecoderPreference.DXVA2 -> {
                    addDxva2()
                    addSoftware()
                }

                DecoderPreference.NVDEC_CUDA -> {
                    addNvdec()
                    addSoftware()
                }

                DecoderPreference.SOFTWARE -> {
                    addSoftware()
                }
            }
        } else {
            addSoftware()
        }

        return attempts.distinctBy { it.label }
    }

    private fun nvidiaDecoderName(codecMimeType: String): String? {
        return when {
            codecMimeType.equals("video/hevc", ignoreCase = true) -> "hevc_cuvid"
            codecMimeType.equals("video/avc", ignoreCase = true) -> "h264_cuvid"
            else -> null
        }
    }

    private fun ffmpegFormat(codecMimeType: String): String {
        return when {
            codecMimeType.equals("video/hevc", ignoreCase = true) -> "hevc"
            else -> "h264"
        }
    }

    private fun codecLabel(codecMimeType: String): String {
        return when {
            codecMimeType.equals("video/hevc", ignoreCase = true) -> "H.265 / HEVC"
            else -> "H.264 / AVC"
        }
    }

    private fun Frame.hasDisplayableImage(): Boolean {
        val imagePlanes = image ?: return false
        return imagePlanes.isNotEmpty() && imagePlanes.any { plane -> plane != null }
    }

    private data class DecoderAttempt(
        val label: String,
        val videoCodecName: String? = null,
        val options: Map<String, String> = emptyMap(),
        val videoOptions: Map<String, String> = emptyMap(),
        val requiresDisplayableFrames: Boolean = false,
    )
}
