package com.everty.evertygame.stream

import android.content.Context
import android.content.Intent
import android.hardware.display.DisplayManager
import android.hardware.display.VirtualDisplay
import android.media.MediaCodec
import android.media.MediaCodecInfo
import android.media.MediaFormat
import android.media.projection.MediaProjection
import android.media.projection.MediaProjectionManager
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.HandlerThread
import android.os.SystemClock
import android.util.DisplayMetrics
import android.view.Display
import android.view.Surface
import com.everty.evertygame.input.GamepadBoostSupport
import com.everty.evertygame.touch.TouchLatencySprintController
import java.io.ByteArrayOutputStream
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.util.LinkedHashMap
import kotlin.math.roundToInt
import kotlin.math.sqrt

class ScreenCaptureStreamer(
    private val context: Context,
    private val config: StreamConfig,
    private val projectionManager: MediaProjectionManager,
    private val resultCode: Int,
    private val projectionData: Intent,
    private val onStreamingStarted: (resolutionLabel: String) -> Unit,
    private val onMetricsUpdated: (StreamMetrics) -> Unit,
    private val onFatalError: (String) -> Unit,
) {
    private val packetizer = H264Packetizer()
    private val codecThread = HandlerThread("EvertyEncoder")
    private val codecHandler by lazy { Handler(codecThread.looper) }
    private val mainHandler by lazy { Handler(context.mainLooper) }
    private val displayManager by lazy { context.getSystemService(DisplayManager::class.java) }

    private var packetSender: PacketSender? = null
    private var mediaProjection: MediaProjection? = null
    private var videoEncoder: MediaCodec? = null
    private var enhancementEncoder: MediaCodec? = null
    private var virtualDisplay: VirtualDisplay? = null
    private var splitStreamHub: SplitStreamGlHub? = null
    private var audioStreamer: PlaybackAudioStreamer? = null
    private var gamepadMonitor: GamepadBoostSupport.Monitor? = null
    private var latestCodecConfig: ByteArray? = null
    private var latestEnhancementCodecConfig: ByteArray? = null
    private var frameSequence = 0
    private var enhancementFrameSequence = 0
    private var isStopping = false
    private var gamepadConnected = false
    private var splitStreamEnabled = false
    private var baseEncoderSupportsRoi = false
    private var lastAppliedRoiGeneration = 0L
    private var splitStreamRoiActive = false
    private var enhancementSurfaceSize = 0
    private val pendingEnhancementMetadata = LinkedHashMap<Long, PendingEnhancementMetadata>()

    @Volatile
    private var fatalErrorReported = false

    private var totalFramesSent = 0L
    private var totalPacketsSent = 0L
    private var totalDroppedFrames = 0L
    private var bytesSentInWindow = 0L
    private var framesSentInWindow = 0L
    private var lastMetricsTimestampMs = 0L
    private var lastPipelineLatencyMs = 0
    private var resolutionLabel = "-"
    private var activeWidth = 0
    private var activeHeight = 0
    private var activeFps = 0
    private var baseBitrateBps = 0
    private var currentBitrateBps = 0
    private var minBitrateBps = 0
    private var criticalMinBitrateBps = 0
    private var lastAdaptiveActionAtMs = 0L
    private var lastHighPressureAtMs = 0L
    private var lastTransportDropAtMs = 0L
    private var lastSyncFrameRequestAtMs = 0L
    private var lastComplexFrameAtMs = 0L
    private var motionHeadroomUntilMs = 0L
    private var recoveryModeUntilMs = 0L
    private var keyframeOnlyRecoveryUntilMs = 0L
    private var latencySprintUntilMs = 0L
    private var lastExternalTouchSprintUntilMs = 0L
    private var lastExternalTouchPulseGeneration = 0L
    private var lastGamepadBoostPulseAtMs = 0L
    private var lowCadenceSinceMs = 0L
    private var lastCadenceTrimAtMs = 0L
    private var lastDisplayRotation = Surface.ROTATION_0
    private var lastDisplayWidth = 0
    private var lastDisplayHeight = 0
    private var lastDisplayDensityDpi = 0
    private var lastVideoReconfigureAtMs = 0L
    private var displayReconfigureScheduled = false

    private val syncFrameRunnable = object : Runnable {
        override fun run() {
            if (isStopping) {
                return
            }
            val intervalMs = nextPeriodicSyncIntervalMs(SystemClock.elapsedRealtime())
            requestSyncFrame(minIntervalMs = (intervalMs / 2).coerceAtLeast(250L))
            scheduleSyncFrameRequest()
        }
    }

    private val displayReconfigureRunnable = Runnable {
        displayReconfigureScheduled = false
        maybeReconfigureVideoCapture()
    }

    private val projectionCallback = object : MediaProjection.Callback() {
        override fun onStop() {
            if (!isStopping) {
                reportFatalError("System screen capture was stopped")
            }
        }
    }

    private val displayListener = object : DisplayManager.DisplayListener {
        override fun onDisplayAdded(displayId: Int) = Unit

        override fun onDisplayRemoved(displayId: Int) = Unit

        override fun onDisplayChanged(displayId: Int) {
            if (displayId != Display.DEFAULT_DISPLAY || isStopping || !codecThread.isAlive) {
                return
            }
            scheduleDisplayReconfigure()
        }
    }

    fun start() {
        try {
            codecThread.start()
            codecHandler.post {
                startStreamingOnWorker()
            }
        } catch (t: Throwable) {
            stop()
            reportFatalError(t.message ?: "Failed to start video pipeline")
        }
    }

    fun stop() {
        if (isStopping) {
            return
        }
        isStopping = true

        if (codecThread.isAlive) {
            runCatching { codecHandler.removeCallbacksAndMessages(null) }
        }
        runCatching { displayManager?.unregisterDisplayListener(displayListener) }
        runCatching { gamepadMonitor?.close() }
        runCatching { videoEncoder?.setCallback(null, null) }
        runCatching { enhancementEncoder?.setCallback(null, null) }
        runCatching { virtualDisplay?.release() }
        runCatching { splitStreamHub?.close() }
        runCatching { videoEncoder?.stop() }
        runCatching { enhancementEncoder?.stop() }
        runCatching { videoEncoder?.release() }
        runCatching { enhancementEncoder?.release() }
        runCatching { audioStreamer?.close() }
        runCatching { mediaProjection?.unregisterCallback(projectionCallback) }
        runCatching { mediaProjection?.stop() }
        runCatching { packetSender?.close() }
        runCatching {
            if (codecThread.isAlive) {
                codecThread.quitSafely()
                if (Thread.currentThread() != codecThread) {
                    codecThread.join(1_000)
                }
            }
        }

        videoEncoder = null
        enhancementEncoder = null
        virtualDisplay = null
        splitStreamHub = null
        audioStreamer = null
        mediaProjection = null
        packetSender = null
        gamepadMonitor = null
    }

    private fun startStreamingOnWorker() {
        try {
            packetSender = createPacketSender()
            val displayInfo = readDisplayInfo(context)
            val captureSize = resolveCaptureSize(
                sourceWidth = displayInfo.width,
                sourceHeight = displayInfo.height,
                preset = config.preset,
            )

            val projection = projectionManager.getMediaProjection(resultCode, projectionData)
                ?: error("Failed to obtain MediaProjection")
            mediaProjection = projection
            projection.registerCallback(projectionCallback, Handler(context.mainLooper))
            registerRuntimeMonitors(displayInfo)

            val configuredEncoder = configureEncoder(
                width = captureSize.width,
                height = captureSize.height,
                preset = config.preset,
            )
            attachConfiguredVideoPipeline(
                projection = projection,
                displayInfo = displayInfo,
                captureSize = captureSize,
                configuredEncoder = configuredEncoder,
            )

            requestSyncFrame()
            scheduleSyncFrameRequest()
            if (config.audioEnabled && Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                runCatching {
                    PlaybackAudioStreamer(
                        mediaProjection = projection,
                        packetizer = packetizer,
                        packetSender = packetSender ?: error("Packet sender is not ready"),
                        onError = {
                            runCatching { audioStreamer?.close() }
                            audioStreamer = null
                        },
                    ).also { streamer ->
                        streamer.start()
                        audioStreamer = streamer
                    }
                }
            }
            lastMetricsTimestampMs = SystemClock.elapsedRealtime()
            mainHandler.post {
                onStreamingStarted(resolutionLabel)
            }
        } catch (t: Throwable) {
            stop()
            reportFatalError(t.message ?: "Failed to start video pipeline")
        }
    }

    private fun registerRuntimeMonitors(displayInfo: DisplayInfo) {
        lastDisplayRotation = displayInfo.rotation
        lastDisplayWidth = displayInfo.width
        lastDisplayHeight = displayInfo.height
        lastDisplayDensityDpi = displayInfo.densityDpi

        runCatching {
            displayManager?.registerDisplayListener(displayListener, codecHandler)
        }

        if (!config.gamepadBoostEnabled) {
            return
        }

        gamepadMonitor?.close()
        gamepadMonitor = GamepadBoostSupport.Monitor(
            context = context,
            handler = mainHandler,
        ) { connected ->
            gamepadConnected = connected
            if (connected && !isStopping && codecThread.isAlive) {
                codecHandler.post {
                    onGamepadBoostConnected()
                }
            }
        }.also { monitor ->
            monitor.start()
        }
    }

    private fun onGamepadBoostConnected() {
        val now = SystemClock.elapsedRealtime()
        latencySprintUntilMs = maxOf(latencySprintUntilMs, now + 180L)
        motionHeadroomUntilMs = maxOf(motionHeadroomUntilMs, now + 260L)
        recoveryModeUntilMs = maxOf(recoveryModeUntilMs, now + 220L)
        requestSyncFrame(minIntervalMs = 70L)
    }

    private fun scheduleDisplayReconfigure() {
        if (displayReconfigureScheduled || isStopping || !codecThread.isAlive) {
            return
        }
        displayReconfigureScheduled = true
        codecHandler.postDelayed(displayReconfigureRunnable, 220L)
    }

    private fun maybeReconfigureVideoCapture() {
        if (isStopping) {
            return
        }

        val now = SystemClock.elapsedRealtime()
        if (now - lastVideoReconfigureAtMs < 450L) {
            scheduleDisplayReconfigure()
            return
        }

        val projection = mediaProjection ?: return
        val displayInfo = readDisplayInfo(context)
        val captureSize = resolveCaptureSize(
            sourceWidth = displayInfo.width,
            sourceHeight = displayInfo.height,
            preset = config.preset,
        )

        val orientationChanged = isLandscape(displayInfo.width, displayInfo.height) != isLandscape(lastDisplayWidth, lastDisplayHeight)
        val sizeDeltaPx = maxOf(
            kotlin.math.abs(displayInfo.width - lastDisplayWidth),
            kotlin.math.abs(displayInfo.height - lastDisplayHeight),
        )
        val displayChanged =
            displayInfo.rotation != lastDisplayRotation ||
                orientationChanged ||
                sizeDeltaPx >= 96 ||
                displayInfo.densityDpi != lastDisplayDensityDpi
        val captureChanged = captureSize.width != activeWidth || captureSize.height != activeHeight
        if (!displayChanged && !captureChanged) {
            return
        }

        val previousEncoder = videoEncoder
        val previousEnhancementEncoder = enhancementEncoder
        val previousVirtualDisplay = virtualDisplay
        latestCodecConfig = null

        runCatching { previousEncoder?.setCallback(null, null) }
        runCatching { previousEnhancementEncoder?.setCallback(null, null) }
        runCatching { previousVirtualDisplay?.release() }
        runCatching { splitStreamHub?.close() }
        runCatching { previousEncoder?.stop() }
        runCatching { previousEnhancementEncoder?.stop() }
        runCatching { previousEncoder?.release() }
        runCatching { previousEnhancementEncoder?.release() }
        videoEncoder = null
        enhancementEncoder = null
        virtualDisplay = null
        splitStreamHub = null

        val configuredEncoder = configureEncoder(
            width = captureSize.width,
            height = captureSize.height,
            preset = config.preset,
        )
        attachConfiguredVideoPipeline(
            projection = projection,
            displayInfo = displayInfo,
            captureSize = captureSize,
            configuredEncoder = configuredEncoder,
        )
        latencySprintUntilMs = maxOf(latencySprintUntilMs, now + 220L)
        requestSyncFrame(minIntervalMs = 0L)
        scheduleSyncFrameRequest()
        lastVideoReconfigureAtMs = now
    }

    private fun attachConfiguredVideoPipeline(
        projection: MediaProjection,
        displayInfo: DisplayInfo,
        captureSize: CaptureSize,
        configuredEncoder: ConfiguredEncoder,
    ) {
        videoEncoder = configuredEncoder.encoder
        splitStreamEnabled = shouldUseSplitStream()
        latestEnhancementCodecConfig = null
        enhancementFrameSequence = 0
        pendingEnhancementMetadata.clear()
        splitStreamRoiActive = false
        lastAppliedRoiGeneration = 0L
        activeWidth = captureSize.width
        activeHeight = captureSize.height
        updateBitrateTargets(configuredEncoder)
        resolutionLabel = "${captureSize.width}x${captureSize.height} @ ${configuredEncoder.fps}"

        val displaySurface = if (splitStreamEnabled) {
            val configuredEnhancementEncoder = configureEnhancementEncoder(captureSize, displayInfo)
            enhancementEncoder = configuredEnhancementEncoder.encoder
            enhancementSurfaceSize = configuredEnhancementEncoder.width
            splitStreamHub = SplitStreamGlHub(
                callbackHandler = codecHandler,
                captureWidth = captureSize.width,
                captureHeight = captureSize.height,
                baseSurfaceWidth = captureSize.width,
                baseSurfaceHeight = captureSize.height,
                enhancementSurfaceWidth = configuredEnhancementEncoder.width,
                enhancementSurfaceHeight = configuredEnhancementEncoder.height,
                screenWidth = displayInfo.width,
                screenHeight = displayInfo.height,
                baseInputSurface = configuredEncoder.inputSurface,
                enhancementInputSurface = configuredEnhancementEncoder.inputSurface,
                roiProvider = {
                    TouchLatencySprintController.currentRoiSnapshot(displayInfo.width, displayInfo.height)
                },
                onEnhancementRendered = { presentationTimeUs, roiSnapshot ->
                    recordEnhancementMetadata(presentationTimeUs, roiSnapshot, displayInfo)
                },
            )
            baseEncoderSupportsRoi = supportsEncoderRoi(configuredEncoder.encoder)
            splitStreamHub?.inputSurface ?: configuredEncoder.inputSurface
        } else {
            enhancementEncoder = null
            splitStreamHub = null
            enhancementSurfaceSize = 0
            baseEncoderSupportsRoi = false
            configuredEncoder.inputSurface
        }

        sendSessionConfigPacket(
            width = captureSize.width,
            height = captureSize.height,
            fps = configuredEncoder.fps,
            bitrateBps = configuredEncoder.bitrateBps,
        )

        virtualDisplay = projection.createVirtualDisplay(
            "EvertyCapture",
            captureSize.width,
            captureSize.height,
            displayInfo.densityDpi,
            DisplayManager.VIRTUAL_DISPLAY_FLAG_AUTO_MIRROR,
            displaySurface,
            null,
            codecHandler,
        )
        applySplitStreamBitrateTargets(currentBitrateBps)

        lastDisplayRotation = displayInfo.rotation
        lastDisplayWidth = displayInfo.width
        lastDisplayHeight = displayInfo.height
        lastDisplayDensityDpi = displayInfo.densityDpi
        mainHandler.post {
            onStreamingStarted(resolutionLabel)
        }
    }

    private fun updateBitrateTargets(configuredEncoder: ConfiguredEncoder) {
        activeFps = configuredEncoder.fps
        baseBitrateBps = configuredEncoder.bitrateBps
        currentBitrateBps = configuredEncoder.bitrateBps
        minBitrateBps = when (config.preset) {
            QualityPreset.TOURNAMENT_FIGHTER,
            QualityPreset.WI_FI_GAMING,
            -> when (config.adaptationMode) {
                AdaptationMode.LOCK_QUALITY -> (configuredEncoder.bitrateBps * 0.82f).toInt().coerceAtLeast(2_000_000)
                AdaptationMode.WI_FI_LAN_TURBO -> (configuredEncoder.bitrateBps * 0.84f).toInt().coerceAtLeast(2_600_000)
                AdaptationMode.LOWEST_LATENCY -> (configuredEncoder.bitrateBps * 0.30f).toInt().coerceAtLeast(850_000)
                AdaptationMode.AUTO_BALANCED -> (configuredEncoder.bitrateBps * 0.45f).toInt().coerceAtLeast(1_200_000)
            }

            QualityPreset.INSTANT_PLAY -> when (config.adaptationMode) {
                AdaptationMode.LOCK_QUALITY -> (configuredEncoder.bitrateBps * 0.75f).toInt().coerceAtLeast(1_400_000)
                AdaptationMode.WI_FI_LAN_TURBO -> (configuredEncoder.bitrateBps * 0.80f).toInt().coerceAtLeast(1_600_000)
                AdaptationMode.LOWEST_LATENCY -> (configuredEncoder.bitrateBps * 0.28f).toInt().coerceAtLeast(700_000)
                AdaptationMode.AUTO_BALANCED -> (configuredEncoder.bitrateBps * 0.32f).toInt().coerceAtLeast(850_000)
            }

            else -> when (config.adaptationMode) {
                AdaptationMode.LOCK_QUALITY -> (configuredEncoder.bitrateBps * 0.70f).toInt().coerceAtLeast(1_500_000)
                AdaptationMode.WI_FI_LAN_TURBO -> (configuredEncoder.bitrateBps * 0.78f).toInt().coerceAtLeast(2_000_000)
                AdaptationMode.LOWEST_LATENCY -> (configuredEncoder.bitrateBps * 0.22f).toInt().coerceAtLeast(650_000)
                AdaptationMode.AUTO_BALANCED -> (configuredEncoder.bitrateBps * 0.30f).toInt().coerceAtLeast(900_000)
            }
        }
        criticalMinBitrateBps = when (config.preset) {
            QualityPreset.TOURNAMENT_FIGHTER,
            QualityPreset.WI_FI_GAMING,
            -> when (config.adaptationMode) {
                AdaptationMode.LOCK_QUALITY -> (configuredEncoder.bitrateBps * 0.56f).toInt().coerceAtLeast(2_200_000)
                AdaptationMode.WI_FI_LAN_TURBO -> (configuredEncoder.bitrateBps * 0.62f).toInt().coerceAtLeast(2_000_000)
                AdaptationMode.LOWEST_LATENCY -> (configuredEncoder.bitrateBps * 0.18f).toInt().coerceAtLeast(700_000)
                AdaptationMode.AUTO_BALANCED -> (configuredEncoder.bitrateBps * 0.24f).toInt().coerceAtLeast(950_000)
            }

            QualityPreset.INSTANT_PLAY -> when (config.adaptationMode) {
                AdaptationMode.LOCK_QUALITY -> (configuredEncoder.bitrateBps * 0.48f).toInt().coerceAtLeast(1_000_000)
                AdaptationMode.WI_FI_LAN_TURBO -> (configuredEncoder.bitrateBps * 0.55f).toInt().coerceAtLeast(900_000)
                AdaptationMode.LOWEST_LATENCY -> (configuredEncoder.bitrateBps * 0.16f).toInt().coerceAtLeast(500_000)
                AdaptationMode.AUTO_BALANCED -> (configuredEncoder.bitrateBps * 0.22f).toInt().coerceAtLeast(650_000)
            }

            else -> when (config.adaptationMode) {
                AdaptationMode.LOCK_QUALITY -> (configuredEncoder.bitrateBps * 0.45f).toInt().coerceAtLeast(1_400_000)
                AdaptationMode.WI_FI_LAN_TURBO -> (configuredEncoder.bitrateBps * 0.58f).toInt().coerceAtLeast(1_500_000)
                AdaptationMode.LOWEST_LATENCY -> (configuredEncoder.bitrateBps * 0.16f).toInt().coerceAtLeast(500_000)
                AdaptationMode.AUTO_BALANCED -> (configuredEncoder.bitrateBps * 0.22f).toInt().coerceAtLeast(700_000)
            }
        }
        if (config.transport == StreamTransport.ADB_TUNNEL_TCP) {
            minBitrateBps = maxOf(minBitrateBps, (configuredEncoder.bitrateBps * 0.72f).toInt())
            criticalMinBitrateBps = maxOf(criticalMinBitrateBps, (configuredEncoder.bitrateBps * 0.55f).toInt())
        }
    }

    private fun configureEncoder(
        width: Int,
        height: Int,
        preset: QualityPreset,
        channel: VideoChannel = VideoChannel.BASE,
    ): ConfiguredEncoder {
        var lastError: Throwable? = null

        for (attempt in buildEncoderAttempts(width, height, preset)) {
            val encoder = MediaCodec.createEncoderByType(config.codec.mimeType)
            try {
                val format = createEncoderFormat(
                    encoder = encoder,
                    width = width,
                    height = height,
                    attempt = attempt,
                )
                encoder.setCallback(createCodecCallback(channel), codecHandler)
                encoder.configure(format, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
                val inputSurface = encoder.createInputSurface()
                encoder.start()
                return ConfiguredEncoder(
                    encoder = encoder,
                    inputSurface = inputSurface,
                    fps = attempt.fps,
                    bitrateBps = attempt.bitrateBps,
                    width = width,
                    height = height,
                )
            } catch (t: Throwable) {
                lastError = t
                runCatching { encoder.stop() }
                runCatching { encoder.release() }
            }
        }

        val message = lastError?.let(::describeEncoderError) ?: "Unknown encoder startup failure"
        error("Encoder start failed: $message")
    }

    private fun configureEnhancementEncoder(
        captureSize: CaptureSize,
        displayInfo: DisplayInfo,
    ): ConfiguredEncoder {
        val size = computeEnhancementSurfaceSize(captureSize, displayInfo)
        val targetBitrate = (config.targetBitrateBps * 0.35f).roundToInt().coerceAtLeast(2_000_000)
        val attempts = listOf(
            EncoderAttempt(
                fps = config.targetFps.coerceIn(24, 120),
                bitrateBps = targetBitrate,
                keyFrameIntervalSeconds = 1,
                operatingRateFps = maxOf(config.targetFps * 1.20f, config.targetFps + 8f),
                avcProfilePreference = AvcProfilePreference.MAIN,
            ),
            EncoderAttempt(
                fps = config.targetFps.coerceIn(24, 120),
                bitrateBps = targetBitrate,
                keyFrameIntervalSeconds = 1,
                operatingRateFps = maxOf(config.targetFps * 1.20f, config.targetFps + 8f),
                avcProfilePreference = AvcProfilePreference.DEFAULT,
            ),
        )
        var lastError: Throwable? = null
        for (attempt in attempts) {
            val encoder = MediaCodec.createEncoderByType(config.codec.mimeType)
            try {
                val format = createEncoderFormat(
                    encoder = encoder,
                    width = size,
                    height = size,
                    attempt = attempt,
                )
                encoder.setCallback(createCodecCallback(VideoChannel.ENHANCEMENT), codecHandler)
                encoder.configure(format, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
                val inputSurface = encoder.createInputSurface()
                encoder.start()
                return ConfiguredEncoder(
                    encoder = encoder,
                    inputSurface = inputSurface,
                    fps = attempt.fps,
                    bitrateBps = attempt.bitrateBps,
                    width = size,
                    height = size,
                )
            } catch (t: Throwable) {
                lastError = t
                runCatching { encoder.stop() }
                runCatching { encoder.release() }
            }
        }

        val message = lastError?.let(::describeEncoderError) ?: "Unknown enhancement encoder startup failure"
        error("Enhancement encoder start failed: $message")
    }

    private fun buildEncoderAttempts(
        width: Int,
        height: Int,
        preset: QualityPreset,
    ): List<EncoderAttempt> {
        val targetFps = config.targetFps.coerceIn(24, 120)
        val targetBitrateBps = config.targetBitrateBps.coerceIn(1_000_000, 100_000_000)
        val fpsCandidates = linkedSetOf(targetFps)
        if (targetFps > 45) {
            fpsCandidates += 45
        }
        if (targetFps > 30) {
            fpsCandidates += 30
        }
        if (targetFps > 24) {
            fpsCandidates += 24
        }

        val attempts = mutableListOf<EncoderAttempt>()
        val referencePixels = (preset.targetWidth * preset.targetHeight).toFloat().coerceAtLeast(1f)
        val capturePixels = (width * height).toFloat().coerceAtLeast(1f)
        val areaScale = (capturePixels / referencePixels).coerceIn(0.74f, 1.0f)
        val transportBitrateFactor = when (config.transport) {
            StreamTransport.UDP -> when (preset) {
                QualityPreset.TOURNAMENT_FIGHTER -> if (config.adaptationMode == AdaptationMode.WI_FI_LAN_TURBO) 1.55f else 0.92f
                QualityPreset.WI_FI_GAMING -> if (config.adaptationMode == AdaptationMode.WI_FI_LAN_TURBO) 1.25f else 1.0f
                QualityPreset.COMPETITIVE -> if (config.adaptationMode == AdaptationMode.WI_FI_LAN_TURBO) 1.45f else 0.96f
                QualityPreset.BALANCED_LOW_LATENCY -> if (config.adaptationMode == AdaptationMode.WI_FI_LAN_TURBO) 1.30f else 1.0f
                QualityPreset.INSTANT_PLAY -> if (config.adaptationMode == AdaptationMode.WI_FI_LAN_TURBO) 1.15f else 1.0f
                else -> 1.0f
            }

            StreamTransport.ADB_TUNNEL_TCP -> 1.0f
        }
        val effectivePresetBitrate = (targetBitrateBps * areaScale * transportBitrateFactor)
            .roundToInt()
            .coerceAtLeast(1_400_000)
        val operatingRateMultiplier = when (config.transport) {
            StreamTransport.UDP -> when (preset) {
                QualityPreset.TOURNAMENT_FIGHTER,
                QualityPreset.COMPETITIVE,
                -> 1.45f
                QualityPreset.WI_FI_GAMING -> 1.52f

                QualityPreset.BALANCED_LOW_LATENCY -> 1.30f
                QualityPreset.INSTANT_PLAY -> 1.20f
                else -> 1.12f
            }

            StreamTransport.ADB_TUNNEL_TCP -> 1.0f
        } +
            if (config.adaptationMode == AdaptationMode.WI_FI_LAN_TURBO && config.transport == StreamTransport.UDP) 0.12f else 0f +
            if (config.gamepadBoostEnabled && config.transport == StreamTransport.UDP) 0.10f else 0f
        fpsCandidates.forEachIndexed { index, fps ->
            val bitrate = ((effectivePresetBitrate.toLong() * fps) / targetFps)
                .toInt()
                .coerceAtLeast(1_200_000)
            val operatingRateFps = maxOf(fps.toFloat() * operatingRateMultiplier, fps + 6f)

            if (config.codec == VideoCodec.AVC) {
                if (index == 0) {
                    attempts += EncoderAttempt(
                        fps = fps,
                        bitrateBps = bitrate,
                        keyFrameIntervalSeconds = preset.keyFrameIntervalSeconds,
                        operatingRateFps = operatingRateFps,
                        avcProfilePreference = AvcProfilePreference.MAIN,
                    )
                }

                attempts += EncoderAttempt(
                    fps = fps,
                    bitrateBps = bitrate,
                    keyFrameIntervalSeconds = preset.keyFrameIntervalSeconds,
                    operatingRateFps = operatingRateFps,
                    avcProfilePreference = AvcProfilePreference.DEFAULT,
                )
                attempts += EncoderAttempt(
                    fps = fps,
                    bitrateBps = bitrate,
                    keyFrameIntervalSeconds = preset.keyFrameIntervalSeconds,
                    operatingRateFps = operatingRateFps,
                    avcProfilePreference = AvcProfilePreference.BASELINE,
                )
            } else {
                attempts += EncoderAttempt(
                    fps = fps,
                    bitrateBps = bitrate,
                    keyFrameIntervalSeconds = preset.keyFrameIntervalSeconds,
                    operatingRateFps = operatingRateFps,
                    avcProfilePreference = AvcProfilePreference.DEFAULT,
                )
            }
        }

        return attempts.distinctBy { attempt ->
            "${attempt.fps}:${attempt.bitrateBps}:${attempt.keyFrameIntervalSeconds}:${attempt.operatingRateFps}:${attempt.avcProfilePreference}"
        }
    }

    private fun createCodecCallback(channel: VideoChannel): MediaCodec.Callback {
        return object : MediaCodec.Callback() {
            override fun onInputBufferAvailable(codec: MediaCodec, index: Int) = Unit

            override fun onOutputBufferAvailable(
                codec: MediaCodec,
                index: Int,
                info: MediaCodec.BufferInfo,
            ) {
                val outputBuffer = codec.getOutputBuffer(index)
                if (outputBuffer == null) {
                    codec.releaseOutputBuffer(index, false)
                    return
                }

                if (info.size <= 0) {
                    codec.releaseOutputBuffer(index, false)
                    return
                }

                try {
                    when {
                        info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG != 0 -> {
                            outputBuffer.position(info.offset)
                            outputBuffer.limit(info.offset + info.size)
                            val payload = outputBuffer.copyToByteArray()
                            if (channel == VideoChannel.BASE) {
                                latestCodecConfig = normalizeCodecConfig(payload)
                                latestCodecConfig?.let(::sendCodecConfigPacket)
                            } else {
                                latestEnhancementCodecConfig = normalizeCodecConfig(payload)
                                latestEnhancementCodecConfig?.let(::sendEnhancementCodecConfigPacket)
                            }
                        }

                        else -> {
                            val isKeyFrame = info.flags and MediaCodec.BUFFER_FLAG_KEY_FRAME != 0
                            if (isKeyFrame && channel == VideoChannel.BASE) {
                                latestCodecConfig?.let(::sendCodecConfigPacket)
                            }
                            if (isKeyFrame && channel == VideoChannel.ENHANCEMENT) {
                                latestEnhancementCodecConfig?.let(::sendEnhancementCodecConfigPacket)
                            }
                            val preparedPayload = prepareVideoPayloadForTransport(
                                payloadBuffer = outputBuffer,
                                payloadOffset = info.offset,
                                payloadSize = info.size,
                            )
                            if (channel == VideoChannel.BASE) {
                                val fastVideoSender = packetSender as? FastVideoFrameSender
                                if (
                                    fastVideoSender != null &&
                                    config.transport == StreamTransport.UDP &&
                                    outputBuffer.isDirect &&
                                    preparedPayload.normalizedBytes == null
                                ) {
                                    sendVideoFrameDirect(
                                        sender = fastVideoSender,
                                        payloadBuffer = outputBuffer,
                                        payloadOffset = preparedPayload.payloadOffset,
                                        payloadSize = preparedPayload.payloadSize,
                                        presentationTimeUs = info.presentationTimeUs,
                                        isKeyFrame = isKeyFrame,
                                    )
                                } else {
                                    sendVideoFrame(
                                        payload = preparedPayload.normalizedBytes ?: run {
                                            outputBuffer.position(preparedPayload.payloadOffset)
                                            outputBuffer.limit(preparedPayload.payloadOffset + preparedPayload.payloadSize)
                                            outputBuffer.copyToByteArray()
                                        },
                                        presentationTimeUs = info.presentationTimeUs,
                                        isKeyFrame = isKeyFrame,
                                    )
                                }
                            } else {
                                sendEnhancementFrame(
                                    payload = preparedPayload.normalizedBytes ?: run {
                                        outputBuffer.position(preparedPayload.payloadOffset)
                                        outputBuffer.limit(preparedPayload.payloadOffset + preparedPayload.payloadSize)
                                        outputBuffer.copyToByteArray()
                                    },
                                    presentationTimeUs = info.presentationTimeUs,
                                    isKeyFrame = isKeyFrame,
                                )
                            }
                        }
                    }
                } catch (t: Throwable) {
                    totalDroppedFrames += 1
                    reportFatalError(t.message ?: "Failed to send video stream")
                } finally {
                    codec.releaseOutputBuffer(index, false)
                }
            }

            override fun onError(codec: MediaCodec, e: MediaCodec.CodecException) {
                reportFatalError("Encoder error: ${e.diagnosticInfo ?: e.message}")
            }

            override fun onOutputFormatChanged(codec: MediaCodec, format: MediaFormat) {
                if (channel == VideoChannel.BASE) {
                    latestCodecConfig = collectCodecConfig(format)
                    latestCodecConfig?.let(::sendCodecConfigPacket)
                } else {
                    latestEnhancementCodecConfig = collectCodecConfig(format)
                    latestEnhancementCodecConfig?.let(::sendEnhancementCodecConfigPacket)
                }
            }
        }
    }

    private fun sendSessionConfigPacket(
        width: Int,
        height: Int,
        fps: Int,
        bitrateBps: Int,
    ) {
        val payload = buildString {
            append("{")
            append("\"codec\":\"${config.codec.mimeType}\",")
            append("\"preset\":\"${config.preset.name}\",")
            append("\"transport\":\"${config.transport.sessionTag}\",")
            append("\"streamMode\":\"${if (splitStreamEnabled) "split" else "single"}\",")
            append("\"width\":$width,")
            append("\"height\":$height,")
            append("\"baseWidth\":$width,")
            append("\"baseHeight\":$height,")
            append("\"fps\":$fps,")
            append("\"bitrate\":$bitrateBps,")
            append("\"baseBitrate\":$bitrateBps,")
            append("\"enhancementEnabled\":${splitStreamEnabled},")
            append("\"enhancementCodec\":\"${if (splitStreamEnabled) config.codec.mimeType else ""}\",")
            append("\"enhancementMaxWidth\":${if (splitStreamEnabled) enhancementSurfaceSize else 0},")
            append("\"enhancementMaxHeight\":${if (splitStreamEnabled) enhancementSurfaceSize else 0},")
            append("\"roiMode\":\"${if (splitStreamEnabled) "accessibility_bounds" else "none"}\"")
            append("}")
        }.toByteArray(Charsets.UTF_8)

        packetSender?.send(packetizer.buildSessionConfigPacket(payload))
        totalPacketsSent += 1
    }

    private fun sendCodecConfigPacket(payload: ByteArray) {
        packetSender?.send(packetizer.buildCodecConfigPacket(payload))
        totalPacketsSent += 1
    }

    private fun sendEnhancementCodecConfigPacket(payload: ByteArray) {
        packetSender?.send(packetizer.buildEnhancementConfigPacket(payload))
        totalPacketsSent += 1
    }

    private fun sendVideoFrame(
        payload: ByteArray,
        presentationTimeUs: Long,
        isKeyFrame: Boolean,
    ) {
        val now = SystemClock.elapsedRealtime()
        maybeApplyRealtimeBoosts(now)
        val frameId = frameSequence++
        val packets = packetizer.packetizeVideoFrame(
            frameId = frameId,
            presentationTimeUs = presentationTimeUs,
            isKeyFrame = isKeyFrame,
            payload = payload,
        )
        maybeApplyMotionHeadroom(
            now = now,
            packetCount = packets.size,
            payloadSize = payload.size,
            isKeyFrame = isKeyFrame,
        )
        if (shouldDropTransportFrame(frameId = frameId, packetCount = packets.size, isKeyFrame = isKeyFrame)) {
            totalDroppedFrames += 1
            emitMetricsIfNeeded()
            return
        }
        packets.forEach { packet ->
            packetSender?.send(packet)
            totalPacketsSent += 1
            bytesSentInWindow += packet.size
        }
        totalFramesSent += 1
        framesSentInWindow += 1
        lastPipelineLatencyMs = ((System.nanoTime() / 1_000 - presentationTimeUs).coerceAtLeast(0L) / 1_000L).toInt()
        emitMetricsIfNeeded()
    }

    private fun sendEnhancementFrame(
        payload: ByteArray,
        presentationTimeUs: Long,
        isKeyFrame: Boolean,
    ) {
        if (!splitStreamEnabled) {
            return
        }

        val metadata = takeEnhancementMetadata(presentationTimeUs) ?: return
        val frameId = enhancementFrameSequence++
        val metadataPayload = buildRoiMetadataPayload(frameId, metadata, presentationTimeUs)
        packetSender?.send(packetizer.buildRoiMetadataPacket(frameId, presentationTimeUs, metadataPayload))
        totalPacketsSent += 1

        val packets = packetizer.packetizeEnhancementFrame(
            frameId = frameId,
            presentationTimeUs = presentationTimeUs,
            isKeyFrame = isKeyFrame,
            payload = payload,
        )
        packets.forEach { packet ->
            packetSender?.send(packet)
            totalPacketsSent += 1
            bytesSentInWindow += packet.size
        }
    }

    private fun sendVideoFrameDirect(
        sender: FastVideoFrameSender,
        payloadBuffer: ByteBuffer,
        payloadOffset: Int,
        payloadSize: Int,
        presentationTimeUs: Long,
        isKeyFrame: Boolean,
    ) {
        require(payloadSize > 0) { "Video payload must not be empty" }

        val now = SystemClock.elapsedRealtime()
        maybeApplyRealtimeBoosts(now)
        val frameId = frameSequence++
        val packetCount = ((payloadSize + TransportProtocol.MAX_PAYLOAD_SIZE - 1) / TransportProtocol.MAX_PAYLOAD_SIZE)
            .coerceAtLeast(1)
        maybeApplyMotionHeadroom(
            now = now,
            packetCount = packetCount,
            payloadSize = payloadSize,
            isKeyFrame = isKeyFrame,
        )
        if (shouldDropTransportFrame(frameId = frameId, packetCount = packetCount, isKeyFrame = isKeyFrame)) {
            totalDroppedFrames += 1
            emitMetricsIfNeeded()
            return
        }

        val packetsSent = sender.sendVideoFrameDirect(
            frameId = frameId,
            presentationTimeUs = presentationTimeUs,
            isKeyFrame = isKeyFrame,
            payloadBuffer = payloadBuffer,
            payloadOffset = payloadOffset,
            payloadSize = payloadSize,
        )
        require(packetsSent > 0) { "Native UDP sender rejected encoded frame" }

        totalPacketsSent += packetsSent.toLong()
        bytesSentInWindow += payloadSize.toLong() + packetsSent.toLong() * TransportProtocol.HEADER_SIZE
        totalFramesSent += 1
        framesSentInWindow += 1
        lastPipelineLatencyMs = ((System.nanoTime() / 1_000 - presentationTimeUs).coerceAtLeast(0L) / 1_000L).toInt()
        emitMetricsIfNeeded()
    }

    private fun emitMetricsIfNeeded() {
        val now = SystemClock.elapsedRealtime()
        val elapsed = now - lastMetricsTimestampMs
        if (elapsed < 1_000) {
            return
        }

        val fps = ((framesSentInWindow * 1_000.0) / elapsed).roundToInt()
        val bitrateKbps = ((bytesSentInWindow * 8.0) / elapsed).roundToInt()

        onMetricsUpdated(
            StreamMetrics(
                fps = fps,
                bitrateKbps = bitrateKbps,
                pipelineLatencyMs = lastPipelineLatencyMs,
                framesSent = totalFramesSent,
                packetsSent = totalPacketsSent,
                droppedFrames = totalDroppedFrames,
                resolutionLabel = resolutionLabel,
            ),
        )

        framesSentInWindow = 0
        bytesSentInWindow = 0L
        lastMetricsTimestampMs = now
    }

    private fun requestSyncFrame(minIntervalMs: Long = 120L) {
        val encoder = videoEncoder ?: return
        val now = SystemClock.elapsedRealtime()
        if (now - lastSyncFrameRequestAtMs < minIntervalMs) {
            return
        }
        lastSyncFrameRequestAtMs = now
        runCatching {
            encoder.setParameters(
                Bundle().apply {
                    putInt(MediaCodec.PARAMETER_KEY_REQUEST_SYNC_FRAME, 0)
                },
            )
        }
        runCatching {
            enhancementEncoder?.setParameters(
                Bundle().apply {
                    putInt(MediaCodec.PARAMETER_KEY_REQUEST_SYNC_FRAME, 0)
                },
            )
        }
    }

    private fun shouldDropTransportFrame(frameId: Int, packetCount: Int, isKeyFrame: Boolean): Boolean {
        if (isKeyFrame) {
            return false
        }

        val now = SystemClock.elapsedRealtime()
        val motionHeadroomActive = now < motionHeadroomUntilMs
        val keyframeOnlyRecovery = now < keyframeOnlyRecoveryUntilMs
        val latencySprintActive = now < latencySprintUntilMs
        if (keyframeOnlyRecovery) {
            if (now - lastTransportDropAtMs >= 80) {
                lastTransportDropAtMs = now
                requestSyncFrame()
            }
            return true
        }

        if (config.transport == StreamTransport.ADB_TUNNEL_TCP) {
            val recoveryMode = now < recoveryModeUntilMs
            if (!recoveryMode) {
                return false
            }

            val allowInterFrame = packetCount <= 3 && frameId % 3 == 0
            if (!allowInterFrame) {
                if (now - lastTransportDropAtMs >= 100) {
                    lastTransportDropAtMs = now
                    requestSyncFrame()
                }
                return true
            }
            return false
        }

        val recentPressure = now - lastHighPressureAtMs <= 1_500
        val recoveryMode = now < recoveryModeUntilMs

        if (recoveryMode) {
            val allowInterFrame = packetCount <= (if (motionHeadroomActive || latencySprintActive) 4 else 2) && frameId % 3 == 0
            if (!allowInterFrame) {
                if (now - lastTransportDropAtMs >= if (latencySprintActive) 90 else 120) {
                    lastTransportDropAtMs = now
                    requestSyncFrame()
                }
                return true
            }
        }

        if (!recentPressure) {
            return false
        }

        val maxPackets = when (config.preset) {
            QualityPreset.TOURNAMENT_FIGHTER -> if (motionHeadroomActive || latencySprintActive) 11 else 8
            QualityPreset.INSTANT_PLAY -> 6
            QualityPreset.COMPETITIVE -> if (motionHeadroomActive || latencySprintActive) 12 else 9
            else -> 10
        }

        if (packetCount <= maxPackets) {
            return false
        }

        if (now - lastTransportDropAtMs >= if (latencySprintActive) 90 else if (recoveryMode) 120 else 200) {
            lastTransportDropAtMs = now
            if (recentPressure || recoveryMode) {
                requestSyncFrame()
            }
        }
        return true
    }

    private fun maybeApplyMotionHeadroom(
        now: Long,
        packetCount: Int,
        payloadSize: Int,
        isKeyFrame: Boolean,
    ) {
        val complexPacketThreshold = when (config.preset) {
            QualityPreset.TOURNAMENT_FIGHTER -> 7
            QualityPreset.COMPETITIVE -> 8
            QualityPreset.INSTANT_PLAY -> 5
            else -> 6
        }
        val complexPayloadThresholdBytes = when (config.preset) {
            QualityPreset.TOURNAMENT_FIGHTER,
            QualityPreset.COMPETITIVE,
            QualityPreset.BALANCED_LOW_LATENCY,
            -> 28 * 1024

            QualityPreset.INSTANT_PLAY -> 14 * 1024
            else -> 20 * 1024
        }
        val isComplexFrame = isKeyFrame || packetCount >= complexPacketThreshold || payloadSize >= complexPayloadThresholdBytes
        val calmWindowMs = if (config.transport == StreamTransport.ADB_TUNNEL_TCP) 260L else 180L
        val startedMovingAfterCalm =
            !isKeyFrame &&
                packetCount >= complexPacketThreshold &&
                payloadSize >= complexPayloadThresholdBytes &&
                now - lastComplexFrameAtMs >= calmWindowMs

        if (startedMovingAfterCalm) {
            motionHeadroomUntilMs = maxOf(
                motionHeadroomUntilMs,
                now + if (config.transport == StreamTransport.ADB_TUNNEL_TCP) 500L else 420L,
            )

            val headroomTargetBitrate = when (config.preset) {
                QualityPreset.TOURNAMENT_FIGHTER -> (baseBitrateBps * 0.94f).toInt()
                QualityPreset.COMPETITIVE -> (baseBitrateBps * 0.96f).toInt()
                QualityPreset.INSTANT_PLAY -> (baseBitrateBps * 0.90f).toInt()
                else -> (baseBitrateBps * 0.92f).toInt()
            }
            if (currentBitrateBps < headroomTargetBitrate && now - lastAdaptiveActionAtMs >= 140L) {
                applyEncoderBitrate(headroomTargetBitrate)
                lastAdaptiveActionAtMs = now
            }
        }

        if (isComplexFrame) {
            lastComplexFrameAtMs = now
        }
    }

    private fun maybeApplyRealtimeBoosts(now: Long) {
        maybeApplyExternalTouchLatencySprint(now)
        maybeApplyGamepadBoost(now)
        maybeUpdateSplitStreamState(now)
    }

    private fun maybeUpdateSplitStreamState(now: Long) {
        if (!splitStreamEnabled) {
            return
        }

        val roiSnapshot = TouchLatencySprintController.currentRoiSnapshot(
            screenWidth = lastDisplayWidth.coerceAtLeast(1),
            screenHeight = lastDisplayHeight.coerceAtLeast(1),
            now = now,
        )
        val roiActive = roiSnapshot?.warmUntilElapsedRealtimeMs?.let { it > now } == true
        if (roiSnapshot != null && roiSnapshot.generation != lastAppliedRoiGeneration) {
            lastAppliedRoiGeneration = roiSnapshot.generation
            requestSyncFrame(minIntervalMs = 0L)
            applyBaseEncoderRoi(roiSnapshot)
        }

        if (splitStreamRoiActive != roiActive) {
            splitStreamRoiActive = roiActive
            applySplitStreamBitrateTargets(currentBitrateBps)
        }
    }

    private fun maybeApplyExternalTouchLatencySprint(now: Long) {
        if (!config.touchLatencySprintEnabled) {
            return
        }

        val pulse = TouchLatencySprintController.currentPulseSnapshot(now)
        val externalSprintUntilMs = pulse.untilElapsedRealtimeMs
        if (
            externalSprintUntilMs <= now ||
            (externalSprintUntilMs <= lastExternalTouchSprintUntilMs && pulse.generation <= lastExternalTouchPulseGeneration)
        ) {
            return
        }
        lastExternalTouchSprintUntilMs = externalSprintUntilMs
        lastExternalTouchPulseGeneration = pulse.generation

        val pulseLevel = when {
            gamepadConnected -> (pulse.intensity + 1).coerceAtMost(3)
            else -> pulse.intensity.coerceAtLeast(1)
        }
        val pulseLatencyMs = when (pulseLevel) {
            3 -> if (config.transport == StreamTransport.ADB_TUNNEL_TCP) 240L else 220L
            2 -> if (config.transport == StreamTransport.ADB_TUNNEL_TCP) 220L else 190L
            else -> if (config.transport == StreamTransport.ADB_TUNNEL_TCP) 190L else 160L
        }
        val pulseRecoveryMs = when (pulseLevel) {
            3 -> if (config.transport == StreamTransport.ADB_TUNNEL_TCP) 260L else 220L
            2 -> if (config.transport == StreamTransport.ADB_TUNNEL_TCP) 230L else 190L
            else -> if (config.transport == StreamTransport.ADB_TUNNEL_TCP) 200L else 160L
        }
        val pulseMotionMs = when (pulseLevel) {
            3 -> if (config.transport == StreamTransport.ADB_TUNNEL_TCP) 320L else 280L
            2 -> if (config.transport == StreamTransport.ADB_TUNNEL_TCP) 280L else 240L
            else -> if (config.transport == StreamTransport.ADB_TUNNEL_TCP) 240L else 210L
        }

        latencySprintUntilMs = maxOf(latencySprintUntilMs, now + pulseLatencyMs, externalSprintUntilMs)
        motionHeadroomUntilMs = maxOf(
            motionHeadroomUntilMs,
            now + pulseMotionMs,
        )
        recoveryModeUntilMs = maxOf(
            recoveryModeUntilMs,
            now + pulseRecoveryMs,
        )
        if (pulseLevel >= 2) {
            keyframeOnlyRecoveryUntilMs = maxOf(
                keyframeOnlyRecoveryUntilMs,
                now + if (pulseLevel >= 3) 110L else 75L,
            )
        }

        if (currentBitrateBps > minBitrateBps && now - lastAdaptiveActionAtMs >= 120L) {
            val sprintTrimFactor = when (config.adaptationMode) {
                AdaptationMode.LOCK_QUALITY -> when (pulseLevel) {
                    3 -> 0.90f
                    2 -> 0.93f
                    else -> 0.96f
                }

                AdaptationMode.WI_FI_LAN_TURBO -> when (pulseLevel) {
                    3 -> 0.91f
                    2 -> 0.94f
                    else -> 0.97f
                }

                AdaptationMode.LOWEST_LATENCY -> when (pulseLevel) {
                    3 -> 0.82f
                    2 -> 0.86f
                    else -> 0.90f
                }

                AdaptationMode.AUTO_BALANCED -> when (pulseLevel) {
                    3 -> 0.87f
                    2 -> 0.90f
                    else -> 0.93f
                }
            }
            val targetBitrate = (currentBitrateBps * sprintTrimFactor)
                .toInt()
                .coerceAtLeast(minBitrateBps)
            if (targetBitrate < currentBitrateBps) {
                applyEncoderBitrate(targetBitrate)
                lastAdaptiveActionAtMs = now
            }
        }

        requestSyncFrame(
            minIntervalMs = when (pulseLevel) {
                3 -> if (config.transport == StreamTransport.ADB_TUNNEL_TCP) 70L else 35L
                2 -> if (config.transport == StreamTransport.ADB_TUNNEL_TCP) 85L else 50L
                else -> if (config.transport == StreamTransport.ADB_TUNNEL_TCP) 100L else 70L
            },
        )
    }

    private fun maybeApplyGamepadBoost(now: Long) {
        if (!config.gamepadBoostEnabled || !gamepadConnected) {
            return
        }
        if (now - lastGamepadBoostPulseAtMs < 420L) {
            return
        }
        lastGamepadBoostPulseAtMs = now

        motionHeadroomUntilMs = maxOf(motionHeadroomUntilMs, now + 220L)
        recoveryModeUntilMs = maxOf(recoveryModeUntilMs, now + 120L)
    }

    private fun handleControlPacket(payload: ByteArray) {
        when (val message = ControlMessage.parse(payload)) {
            ControlMessage.RequestKeyFrame -> {
                codecHandler.post(::requestSyncFrame)
            }

            is ControlMessage.ReceiverFeedback -> {
                codecHandler.post {
                    handleReceiverFeedback(message)
                }
            }

            null -> Unit
        }
    }

    private fun createPacketSender(): PacketSender {
        val onTransportError = { message: String ->
            reportFatalError(message)
        }
        return when (config.transport) {
            StreamTransport.UDP -> runCatching {
                NativeUdpSender(
                    host = config.host,
                    port = config.port,
                    onControlPacket = ::handleControlPacket,
                    onTransportError = onTransportError,
                )
            }.getOrElse {
                UdpSender(
                    host = config.host,
                    port = config.port,
                    onControlPacket = ::handleControlPacket,
                    onTransportError = onTransportError,
                )
            }

            StreamTransport.ADB_TUNNEL_TCP -> TcpTunnelSender(
                host = config.host,
                port = config.port,
                onControlPacket = ::handleControlPacket,
                onTransportError = onTransportError,
            )
        }
    }

    private fun handleReceiverFeedback(message: ControlMessage.ReceiverFeedback) {
        if (isStopping) {
            return
        }

        val now = SystemClock.elapsedRealtime()
        val gamepadBoostActive = config.gamepadBoostEnabled && gamepadConnected
        val targetCadenceFloor = when {
            activeFps >= 60 -> if (gamepadBoostActive) 58 else 57
            activeFps >= 45 -> (activeFps * 0.93f).toInt()
            else -> (activeFps * 0.90f).toInt()
        }.coerceAtLeast(1)
        val highAssemblyDelayMs = if (config.transport == StreamTransport.ADB_TUNNEL_TCP) 12 else 8
        val criticalAssemblyDelayMs = if (config.transport == StreamTransport.ADB_TUNNEL_TCP) 20 else 12
        val highPresentDeltaMs = if (config.transport == StreamTransport.ADB_TUNNEL_TCP) 28 else 24
        val criticalPresentDeltaMs = if (config.transport == StreamTransport.ADB_TUNNEL_TCP) 42 else 34
        val highArrivalDeltaMs = if (config.transport == StreamTransport.ADB_TUNNEL_TCP) 18 else 20
        val criticalArrivalDeltaMs = if (config.transport == StreamTransport.ADB_TUNNEL_TCP) 28 else 30
        val latencySpike =
            message.presentDeltaMs >= highPresentDeltaMs ||
                message.decodeDeltaMs >= highPresentDeltaMs ||
                message.arrivalDeltaMs >= highArrivalDeltaMs
        val criticalLatencySpike =
            message.presentDeltaMs >= criticalPresentDeltaMs ||
                message.decodeDeltaMs >= criticalPresentDeltaMs ||
                message.arrivalDeltaMs >= criticalArrivalDeltaMs
        val lowCadenceWithoutBacklog =
            message.decodeFps in 1 until targetCadenceFloor &&
                message.backlogFrames == 0 &&
                message.assemblyDelayMs < highAssemblyDelayMs &&
                message.presentDeltaMs in 0..12
        if (lowCadenceWithoutBacklog) {
            if (lowCadenceSinceMs == 0L) {
                lowCadenceSinceMs = now
            }
        } else {
            lowCadenceSinceMs = 0L
        }
        when (message.pressure) {
            ControlMessage.PressureLevel.CRITICAL -> {
                lastHighPressureAtMs = now
                lowCadenceSinceMs = 0L
                if (criticalLatencySpike) {
                    latencySprintUntilMs = maxOf(latencySprintUntilMs, now + 520L)
                } else if (latencySpike) {
                    latencySprintUntilMs = maxOf(latencySprintUntilMs, now + 320L)
                }
                recoveryModeUntilMs = maxOf(
                    recoveryModeUntilMs,
                    now + when (config.preset) {
                        QualityPreset.TOURNAMENT_FIGHTER,
                        QualityPreset.COMPETITIVE,
                        QualityPreset.BALANCED_LOW_LATENCY,
                        -> 1_100L

                        else -> 850L
                    },
                )
                val shouldEnterKeyframeOnlyRecovery =
                    message.backlogFrames > 0 || message.assemblyDelayMs >= criticalAssemblyDelayMs
                if (shouldEnterKeyframeOnlyRecovery) {
                    keyframeOnlyRecoveryUntilMs = maxOf(
                        keyframeOnlyRecoveryUntilMs,
                        now + when (config.preset) {
                            QualityPreset.TOURNAMENT_FIGHTER,
                            QualityPreset.COMPETITIVE,
                            QualityPreset.BALANCED_LOW_LATENCY,
                            -> if (message.assemblyDelayMs >= criticalAssemblyDelayMs) 240L else 160L

                            else -> if (message.assemblyDelayMs >= criticalAssemblyDelayMs) 180L else 120L
                        },
                    )
                }
                if (now - lastAdaptiveActionAtMs >= 180) {
                    val reductionFactor = when (config.preset) {
                        QualityPreset.TOURNAMENT_FIGHTER,
                        QualityPreset.WI_FI_GAMING,
                        -> when (config.adaptationMode) {
                            AdaptationMode.LOCK_QUALITY -> 0.84f
                            AdaptationMode.WI_FI_LAN_TURBO -> 0.76f
                            AdaptationMode.LOWEST_LATENCY -> 0.56f
                            AdaptationMode.AUTO_BALANCED -> 0.68f
                        }

                        QualityPreset.INSTANT_PLAY -> when (config.adaptationMode) {
                            AdaptationMode.LOCK_QUALITY -> 0.80f
                            AdaptationMode.WI_FI_LAN_TURBO -> 0.72f
                            AdaptationMode.LOWEST_LATENCY -> 0.52f
                            AdaptationMode.AUTO_BALANCED -> 0.64f
                        }

                        else -> when (config.adaptationMode) {
                            AdaptationMode.LOCK_QUALITY -> 0.78f
                            AdaptationMode.WI_FI_LAN_TURBO -> 0.70f
                            AdaptationMode.LOWEST_LATENCY -> 0.50f
                            AdaptationMode.AUTO_BALANCED -> 0.60f
                        }
                    }
                    val adjustedReductionFactor = if (
                        (config.transport == StreamTransport.ADB_TUNNEL_TCP && message.backlogFrames > 0) ||
                        message.assemblyDelayMs >= criticalAssemblyDelayMs ||
                        criticalLatencySpike
                    ) {
                        reductionFactor * 0.88f
                    } else if (latencySpike) {
                        reductionFactor * 0.94f
                    } else {
                        reductionFactor
                    }
                    val reducedBitrate = (currentBitrateBps * adjustedReductionFactor).toInt().coerceAtLeast(criticalMinBitrateBps)
                    if (reducedBitrate < currentBitrateBps) {
                        applyEncoderBitrate(reducedBitrate, floorBitrateBps = criticalMinBitrateBps)
                    }
                    lastAdaptiveActionAtMs = now
                }
                requestSyncFrame()
            }

            ControlMessage.PressureLevel.HIGH -> {
                lastHighPressureAtMs = now
                lowCadenceSinceMs = 0L
                if (criticalLatencySpike) {
                    latencySprintUntilMs = maxOf(latencySprintUntilMs, now + 360L)
                } else if (latencySpike) {
                    latencySprintUntilMs = maxOf(latencySprintUntilMs, now + 220L)
                }
                if (
                    (config.transport == StreamTransport.ADB_TUNNEL_TCP && message.backlogFrames > 0) ||
                    message.assemblyDelayMs >= highAssemblyDelayMs ||
                    latencySpike
                ) {
                    keyframeOnlyRecoveryUntilMs = maxOf(
                        keyframeOnlyRecoveryUntilMs,
                        now + if (message.assemblyDelayMs >= criticalAssemblyDelayMs) 140L else 90L,
                    )
                }
                if (now - lastAdaptiveActionAtMs >= 350) {
                    val reductionFactor = when (config.preset) {
                        QualityPreset.TOURNAMENT_FIGHTER,
                        QualityPreset.WI_FI_GAMING,
                        -> when (config.adaptationMode) {
                            AdaptationMode.LOCK_QUALITY -> 0.97f
                            AdaptationMode.WI_FI_LAN_TURBO -> 0.93f
                            AdaptationMode.LOWEST_LATENCY -> 0.78f
                            AdaptationMode.AUTO_BALANCED -> 0.88f
                        }

                        QualityPreset.INSTANT_PLAY -> when (config.adaptationMode) {
                            AdaptationMode.LOCK_QUALITY -> 0.95f
                            AdaptationMode.WI_FI_LAN_TURBO -> 0.88f
                            AdaptationMode.LOWEST_LATENCY -> 0.74f
                            AdaptationMode.AUTO_BALANCED -> 0.82f
                        }

                        else -> when (config.adaptationMode) {
                            AdaptationMode.LOCK_QUALITY -> 0.94f
                            AdaptationMode.WI_FI_LAN_TURBO -> 0.86f
                            AdaptationMode.LOWEST_LATENCY -> 0.72f
                            AdaptationMode.AUTO_BALANCED -> 0.80f
                        }
                    }
                    val adjustedReductionFactor = if (criticalLatencySpike) {
                        reductionFactor * 0.86f
                    } else if (latencySpike) {
                        reductionFactor * 0.93f
                    } else {
                        reductionFactor
                    }
                    val reducedBitrate = (currentBitrateBps * adjustedReductionFactor).toInt().coerceAtLeast(minBitrateBps)
                    if (reducedBitrate < currentBitrateBps) {
                        applyEncoderBitrate(reducedBitrate)
                    }
                    lastAdaptiveActionAtMs = now
                }
                if (message.backlogFrames > 0 || message.assemblyDelayMs >= highAssemblyDelayMs || latencySpike) {
                    requestSyncFrame()
                }
            }

            ControlMessage.PressureLevel.NORMAL -> {
                val stableForMs = now - lastHighPressureAtMs
                val activeMotion = now - lastComplexFrameAtMs < if (config.transport == StreamTransport.UDP) 650L else 900L
                val restoreDelayMs = when (config.adaptationMode) {
                    AdaptationMode.LOCK_QUALITY -> 1_200L
                    AdaptationMode.WI_FI_LAN_TURBO -> 750L
                    AdaptationMode.LOWEST_LATENCY -> 4_000L
                    AdaptationMode.AUTO_BALANCED -> 2_500L
                }
                if (!activeMotion &&
                    stableForMs >= restoreDelayMs &&
                    now - lastAdaptiveActionAtMs >= 2_000 &&
                    currentBitrateBps < baseBitrateBps
                ) {
                    val increaseFactor = when (config.adaptationMode) {
                        AdaptationMode.LOCK_QUALITY -> 1.18f
                        AdaptationMode.WI_FI_LAN_TURBO -> 1.20f
                        AdaptationMode.LOWEST_LATENCY -> 1.04f
                        AdaptationMode.AUTO_BALANCED -> 1.08f
                    }
                    val increasedBitrate = (currentBitrateBps * increaseFactor).toInt().coerceAtMost(baseBitrateBps)
                    if (increasedBitrate > currentBitrateBps) {
                        applyEncoderBitrate(increasedBitrate)
                    }
                    lastAdaptiveActionAtMs = now
                }
                val cadenceTrimWindowMs = when {
                    gamepadBoostActive && activeMotion -> 380L
                    gamepadBoostActive -> 650L
                    activeMotion -> 700L
                    else -> 1_200L
                }
                val cadenceTrimCooldownMs = when {
                    gamepadBoostActive && activeMotion -> 520L
                    gamepadBoostActive -> 900L
                    activeMotion -> 900L
                    else -> 1_500L
                }
                val canTrimForCadence =
                    config.transport == StreamTransport.UDP &&
                        lowCadenceSinceMs != 0L &&
                        now - lowCadenceSinceMs >= cadenceTrimWindowMs &&
                        now - lastCadenceTrimAtMs >= cadenceTrimCooldownMs &&
                        currentBitrateBps > minBitrateBps &&
                        message.backlogFrames == 0 &&
                        message.assemblyDelayMs < highAssemblyDelayMs &&
                        !criticalLatencySpike
                if (canTrimForCadence) {
                    val trimFactor = when {
                        gamepadBoostActive && activeMotion && config.adaptationMode == AdaptationMode.WI_FI_LAN_TURBO -> 0.95f
                        gamepadBoostActive && activeMotion && config.adaptationMode == AdaptationMode.LOWEST_LATENCY -> 0.86f
                        gamepadBoostActive && activeMotion -> 0.90f
                        gamepadBoostActive && config.adaptationMode == AdaptationMode.WI_FI_LAN_TURBO -> 0.96f
                        gamepadBoostActive && config.adaptationMode == AdaptationMode.LOWEST_LATENCY -> 0.88f
                        gamepadBoostActive -> 0.92f
                        activeMotion && config.adaptationMode == AdaptationMode.WI_FI_LAN_TURBO -> 0.96f
                        activeMotion && config.adaptationMode == AdaptationMode.LOCK_QUALITY -> 0.96f
                        activeMotion && config.adaptationMode == AdaptationMode.LOWEST_LATENCY -> 0.90f
                        activeMotion -> 0.94f
                        config.adaptationMode == AdaptationMode.WI_FI_LAN_TURBO -> 0.98f
                        config.adaptationMode == AdaptationMode.LOCK_QUALITY -> 0.96f
                        config.adaptationMode == AdaptationMode.LOWEST_LATENCY -> 0.91f
                        else -> 0.94f
                    }
                    val reducedBitrate = (currentBitrateBps * trimFactor).toInt().coerceAtLeast(minBitrateBps)
                    if (reducedBitrate < currentBitrateBps) {
                        applyEncoderBitrate(reducedBitrate)
                        lastCadenceTrimAtMs = now
                        lastAdaptiveActionAtMs = now
                    }
                    lowCadenceSinceMs = now
                }
            }
        }
    }

    private fun applyEncoderBitrate(targetBitrateBps: Int) {
        applyEncoderBitrate(targetBitrateBps, floorBitrateBps = minBitrateBps)
    }

    private fun applyEncoderBitrate(targetBitrateBps: Int, floorBitrateBps: Int) {
        val encoder = videoEncoder ?: return
        val clamped = targetBitrateBps.coerceAtLeast(floorBitrateBps).coerceAtMost(baseBitrateBps)
        if (clamped == currentBitrateBps) {
            return
        }

        runCatching {
            currentBitrateBps = clamped
            applySplitStreamBitrateTargets(clamped)
            sendSessionConfigPacket(
                width = activeWidth,
                height = activeHeight,
                fps = activeFps,
                bitrateBps = currentBitrateBps,
            )
        }
    }

    private fun applySplitStreamBitrateTargets(totalBitrateBps: Int) {
        val baseEncoder = videoEncoder ?: return
        if (!splitStreamEnabled || enhancementEncoder == null) {
            baseEncoder.setParameters(
                Bundle().apply {
                    putInt(MediaCodec.PARAMETER_KEY_VIDEO_BITRATE, totalBitrateBps)
                },
            )
            return
        }

        val baseTarget = if (splitStreamRoiActive) (totalBitrateBps * 0.65f).roundToInt() else totalBitrateBps
        val enhancementTarget = if (splitStreamRoiActive) {
            (totalBitrateBps * 0.35f).roundToInt().coerceAtLeast(1_500_000)
        } else {
            600_000
        }

        baseEncoder.setParameters(
            Bundle().apply {
                putInt(MediaCodec.PARAMETER_KEY_VIDEO_BITRATE, baseTarget)
            },
        )
        enhancementEncoder?.setParameters(
            Bundle().apply {
                putInt(MediaCodec.PARAMETER_KEY_VIDEO_BITRATE, enhancementTarget)
            },
        )
    }

    private fun scheduleSyncFrameRequest() {
        val intervalMs = nextPeriodicSyncIntervalMs(SystemClock.elapsedRealtime()).toInt()
        if (intervalMs <= 0 || isStopping || !codecThread.isAlive) {
            return
        }
        codecHandler.removeCallbacks(syncFrameRunnable)
        codecHandler.postDelayed(syncFrameRunnable, intervalMs.toLong())
    }

    private fun nextPeriodicSyncIntervalMs(now: Long): Long {
        val unstable =
            now < recoveryModeUntilMs ||
                now < keyframeOnlyRecoveryUntilMs ||
                now < latencySprintUntilMs ||
                now - lastHighPressureAtMs <= 1_500L
        if (unstable) {
            return config.preset.syncFrameIntervalMs.toLong().coerceAtLeast(250L)
        }

        return when (config.transport) {
            StreamTransport.ADB_TUNNEL_TCP -> 1_000L
            StreamTransport.UDP -> when (config.preset) {
                QualityPreset.TOURNAMENT_FIGHTER,
                QualityPreset.COMPETITIVE,
                -> 0L

                QualityPreset.BALANCED_LOW_LATENCY,
                -> 0L

                QualityPreset.INSTANT_PLAY -> 1_500L
                else -> 1_500L
            }
        }
    }

    private fun reportFatalError(message: String) {
        if (fatalErrorReported) {
            return
        }
        fatalErrorReported = true
        mainHandler.post {
            onFatalError(message)
        }
    }

    private fun createEncoderFormat(
        encoder: MediaCodec,
        width: Int,
        height: Int,
        attempt: EncoderAttempt,
    ): MediaFormat {
        return MediaFormat.createVideoFormat(config.codec.mimeType, width, height).apply {
            setInteger(MediaFormat.KEY_COLOR_FORMAT, MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface)
            setInteger(MediaFormat.KEY_BIT_RATE, attempt.bitrateBps)
            setInteger(MediaFormat.KEY_BITRATE_MODE, MediaCodecInfo.EncoderCapabilities.BITRATE_MODE_CBR)
            setInteger(MediaFormat.KEY_FRAME_RATE, attempt.fps)
            setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, attempt.keyFrameIntervalSeconds)
            if (config.codec == VideoCodec.AVC && attempt.avcProfilePreference != AvcProfilePreference.DEFAULT) {
                maybeApplyAvcProfile(encoder, this, attempt.avcProfilePreference)
            }
            maybeApplyFastComplexity(encoder, this)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
                setInteger(MediaFormat.KEY_PRIORITY, 0)
                setFloat(MediaFormat.KEY_OPERATING_RATE, attempt.operatingRateFps)
            }
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                setInteger(MediaFormat.KEY_MAX_B_FRAMES, 0)
            }
        }
    }

    private fun maybeApplyAvcProfile(
        encoder: MediaCodec,
        format: MediaFormat,
        preference: AvcProfilePreference,
    ) {
        if (preference == AvcProfilePreference.DEFAULT) {
            return
        }

        val capabilities = runCatching {
            encoder.codecInfo.getCapabilitiesForType(config.codec.mimeType)
        }.getOrNull() ?: return

        val supportedProfiles = capabilities.profileLevels
            .map { profileLevel -> profileLevel.profile }
            .toSet()

        val targetProfile = when (preference) {
            AvcProfilePreference.MAIN -> {
                if (MediaCodecInfo.CodecProfileLevel.AVCProfileMain in supportedProfiles) {
                    MediaCodecInfo.CodecProfileLevel.AVCProfileMain
                } else {
                    null
                }
            }

            AvcProfilePreference.BASELINE -> {
                if (MediaCodecInfo.CodecProfileLevel.AVCProfileBaseline in supportedProfiles) {
                    MediaCodecInfo.CodecProfileLevel.AVCProfileBaseline
                } else {
                    null
                }
            }

            AvcProfilePreference.DEFAULT -> null
        }
        if (targetProfile == null) {
            return
        }

        runCatching {
            format.setInteger(MediaFormat.KEY_PROFILE, targetProfile)
        }
    }

    private fun maybeApplyFastComplexity(
        encoder: MediaCodec,
        format: MediaFormat,
    ) {
        val encoderCapabilities = runCatching {
            encoder.codecInfo.getCapabilitiesForType(config.codec.mimeType).encoderCapabilities
        }.getOrNull() ?: return

        val minimumComplexity = runCatching {
            encoderCapabilities.complexityRange.lower
        }.getOrNull() ?: return

        runCatching {
            format.setInteger(MediaFormat.KEY_COMPLEXITY, minimumComplexity)
        }
    }

    private fun describeEncoderError(error: Throwable): String {
        return when (error) {
            is MediaCodec.CodecException -> error.diagnosticInfo ?: error.message ?: "CodecException"
            else -> error.message ?: error.javaClass.simpleName
        }
    }

    private fun collectCodecConfig(format: MediaFormat): ByteArray? {
        val codecConfigBuffers = (0..2).mapNotNull { index ->
            format.getByteBuffer("csd-$index")?.copyToByteArray()
        }
        if (codecConfigBuffers.isEmpty()) {
            return null
        }

        val output = ByteArrayOutputStream()
        codecConfigBuffers.forEach { buffer ->
            output.write(normalizeCodecConfig(buffer))
        }
        return output.toByteArray()
    }

    private fun normalizeCodecConfig(payload: ByteArray): ByteArray {
        if (payload.size >= 4 &&
            payload[0] == 0.toByte() &&
            payload[1] == 0.toByte() &&
            payload[2] == 0.toByte() &&
            payload[3] == 1.toByte()
        ) {
            return payload
        }

        return byteArrayOf(0, 0, 0, 1) + payload
    }

    private fun prepareVideoPayloadForTransport(
        payloadBuffer: ByteBuffer,
        payloadOffset: Int,
        payloadSize: Int,
    ): PreparedVideoPayload {
        val duplicate = payloadBuffer.duplicate().order(ByteOrder.BIG_ENDIAN)
        duplicate.position(payloadOffset)
        duplicate.limit(payloadOffset + payloadSize)

        if (startsWithAnnexBStartCode(duplicate)) {
            return PreparedVideoPayload(
                payloadOffset = payloadOffset,
                payloadSize = payloadSize,
                normalizedBytes = null,
            )
        }

        val normalized = convertLengthPrefixedAccessUnitToAnnexB(duplicate)
        return PreparedVideoPayload(
            payloadOffset = payloadOffset,
            payloadSize = payloadSize,
            normalizedBytes = normalized ?: duplicate.copyToByteArray(),
        )
    }

    private fun startsWithAnnexBStartCode(buffer: ByteBuffer): Boolean {
        if (buffer.remaining() < 4) {
            return false
        }

        val start = buffer.position()
        return (buffer[start] == 0.toByte() &&
            buffer[start + 1] == 0.toByte() &&
            buffer[start + 2] == 0.toByte() &&
            buffer[start + 3] == 1.toByte()) ||
            (buffer.remaining() >= 3 &&
                buffer[start] == 0.toByte() &&
                buffer[start + 1] == 0.toByte() &&
                buffer[start + 2] == 1.toByte())
    }

    private fun convertLengthPrefixedAccessUnitToAnnexB(buffer: ByteBuffer): ByteArray? {
        val input = buffer.slice().order(ByteOrder.BIG_ENDIAN)
        if (input.remaining() < 4) {
            return null
        }

        val output = ByteArrayOutputStream(input.remaining() + 32)
        while (input.remaining() > 0) {
            if (input.remaining() < 4) {
                return null
            }

            val nalSize = input.int
            if (nalSize <= 0 || nalSize > input.remaining()) {
                return null
            }

            output.write(byteArrayOf(0, 0, 0, 1))
            val nalBytes = ByteArray(nalSize)
            input.get(nalBytes)
            output.write(nalBytes)
        }

        return output.toByteArray()
    }

    private fun ByteBuffer.copyToByteArray(): ByteArray {
        val duplicate = duplicate()
        val bytes = ByteArray(duplicate.remaining())
        duplicate.get(bytes)
        return bytes
    }

    private fun shouldUseSplitStream(): Boolean {
        return config.transport == StreamTransport.UDP &&
            config.codec == VideoCodec.AVC &&
            config.adaptiveRoiSplitStreamEnabled &&
            config.targetBitrateBps >= 6_000_000
    }

    private fun computeEnhancementSurfaceSize(
        captureSize: CaptureSize,
        displayInfo: DisplayInfo,
    ): Int {
        val baseSize = minOf(captureSize.width, captureSize.height, displayInfo.width, displayInfo.height)
        val target = (baseSize * 0.35f).roundToInt().coerceIn(192, 720)
        return alignDimensionForAvc(target, 720)
    }

    private fun recordEnhancementMetadata(
        presentationTimeUs: Long,
        roiSnapshot: TouchLatencySprintController.RoiSnapshot,
        displayInfo: DisplayInfo,
    ) {
        pendingEnhancementMetadata[presentationTimeUs] = PendingEnhancementMetadata(
            x = roiSnapshot.rect.left,
            y = roiSnapshot.rect.top,
            width = roiSnapshot.rect.width(),
            height = roiSnapshot.rect.height(),
            screenWidth = displayInfo.width,
            screenHeight = displayInfo.height,
            pulseKind = roiSnapshot.pulseKind.name.lowercase(),
            generation = roiSnapshot.generation,
        )
        while (pendingEnhancementMetadata.size > 24) {
            val firstKey = pendingEnhancementMetadata.entries.firstOrNull()?.key ?: break
            pendingEnhancementMetadata.remove(firstKey)
        }
    }

    private fun takeEnhancementMetadata(presentationTimeUs: Long): PendingEnhancementMetadata? {
        pendingEnhancementMetadata.remove(presentationTimeUs)?.let { return it }
        val candidate = pendingEnhancementMetadata.entries
            .minByOrNull { entry -> kotlin.math.abs(entry.key - presentationTimeUs) }
        if (candidate != null && kotlin.math.abs(candidate.key - presentationTimeUs) <= 120_000L) {
            pendingEnhancementMetadata.remove(candidate.key)
            return candidate.value
        }
        return null
    }

    private fun buildRoiMetadataPayload(
        frameId: Int,
        metadata: PendingEnhancementMetadata,
        presentationTimeUs: Long,
    ): ByteArray {
        return buildString {
            append("{")
            append("\"frameId\":$frameId,")
            append("\"x\":${metadata.x},")
            append("\"y\":${metadata.y},")
            append("\"width\":${metadata.width},")
            append("\"height\":${metadata.height},")
            append("\"screenWidth\":${metadata.screenWidth},")
            append("\"screenHeight\":${metadata.screenHeight},")
            append("\"presentationTimeUs\":$presentationTimeUs,")
            append("\"pulseKind\":\"${metadata.pulseKind}\"")
            append("}")
        }.toByteArray(Charsets.UTF_8)
    }

    private fun applyBaseEncoderRoi(roiSnapshot: TouchLatencySprintController.RoiSnapshot) {
        if (!baseEncoderSupportsRoi || Build.VERSION.SDK_INT < 35) {
            return
        }

        val encoder = videoEncoder ?: return
        val displayWidth = lastDisplayWidth.coerceAtLeast(1)
        val displayHeight = lastDisplayHeight.coerceAtLeast(1)
        val mappedRect = android.graphics.Rect(
            (roiSnapshot.rect.left / displayWidth.toFloat() * activeWidth).roundToInt().coerceIn(0, activeWidth - 1),
            (roiSnapshot.rect.top / displayHeight.toFloat() * activeHeight).roundToInt().coerceIn(0, activeHeight - 1),
            (roiSnapshot.rect.right / displayWidth.toFloat() * activeWidth).roundToInt().coerceIn(1, activeWidth),
            (roiSnapshot.rect.bottom / displayHeight.toFloat() * activeHeight).roundToInt().coerceIn(1, activeHeight),
        )
        if (mappedRect.isEmpty) {
            return
        }

        runCatching {
            val serialized = MediaFormat.QpOffsetRect.flattenToString(
                listOf(MediaFormat.QpOffsetRect(mappedRect, -6)),
            )
            encoder.setParameters(
                Bundle().apply {
                    putString(MediaCodec.PARAMETER_KEY_QP_OFFSET_RECTS, serialized)
                },
            )
        }
    }

    private fun supportsEncoderRoi(encoder: MediaCodec): Boolean {
        if (Build.VERSION.SDK_INT < 35 || config.codec != VideoCodec.AVC) {
            return false
        }

        return runCatching {
            encoder.codecInfo
                .getCapabilitiesForType(config.codec.mimeType)
                .isFeatureSupported(MediaCodecInfo.CodecCapabilities.FEATURE_Roi)
        }.getOrDefault(false)
    }

    @Suppress("DEPRECATION")
    private fun readDisplayInfo(context: Context): DisplayInfo {
        val display = context.getSystemService(DisplayManager::class.java)
            ?.getDisplay(Display.DEFAULT_DISPLAY)
            ?: error("Default display is not available")

        val metrics = DisplayMetrics().also(display::getRealMetrics)
        return DisplayInfo(
            width = metrics.widthPixels,
            height = metrics.heightPixels,
            densityDpi = metrics.densityDpi,
            rotation = display.rotation,
        )
    }

    private fun resolveCaptureSize(
        sourceWidth: Int,
        sourceHeight: Int,
        preset: QualityPreset,
    ): CaptureSize {
        val sourceLongEdge = maxOf(sourceWidth, sourceHeight).toFloat()
        val sourceShortEdge = minOf(sourceWidth, sourceHeight).toFloat()
        val presetLongEdge = maxOf(preset.targetWidth, preset.targetHeight).toFloat()
        val presetShortEdge = minOf(preset.targetWidth, preset.targetHeight).toFloat()

        val sourceAspect = sourceLongEdge / sourceShortEdge
        val presetAspect = presetLongEdge / presetShortEdge
        val shortEdgePriority = sourceAspect > presetAspect + 0.15f

        val scale = if (shortEdgePriority) {
            minOf(1f, presetShortEdge / sourceShortEdge)
        } else {
            minOf(1f, presetLongEdge / sourceLongEdge, presetShortEdge / sourceShortEdge)
        }

        var width = alignDimensionForAvc((sourceWidth * scale).roundToInt(), sourceWidth)
        var height = alignDimensionForAvc((sourceHeight * scale).roundToInt(), sourceHeight)

        val referencePixels = preset.targetWidth * preset.targetHeight
        val maxPixels = (referencePixels * capturePixelBudgetFactor(preset)).roundToInt()
        val actualPixels = width * height
        if (actualPixels > maxPixels) {
            val areaScale = sqrt(maxPixels.toFloat() / actualPixels.toFloat())
            width = alignDimensionForAvc((width * areaScale).roundToInt(), sourceWidth)
            height = alignDimensionForAvc((height * areaScale).roundToInt(), sourceHeight)
        }
        return CaptureSize(width = width, height = height)
    }

    private fun alignDimensionForAvc(value: Int, sourceBound: Int): Int {
        val capped = value.coerceAtLeast(16).coerceAtMost(sourceBound.coerceAtLeast(16))
        val alignedDown = (capped / 16) * 16
        if (alignedDown >= 16) {
            return alignedDown
        }
        val alignedUp = ((capped + 15) / 16) * 16
        return alignedUp.coerceAtMost(sourceBound.coerceAtLeast(16)).coerceAtLeast(16)
    }

    private fun isLandscape(width: Int, height: Int): Boolean = width >= height

    private fun capturePixelBudgetFactor(preset: QualityPreset): Float {
        val baseFactor = when (config.transport) {
            StreamTransport.UDP -> when (preset) {
                QualityPreset.TOURNAMENT_FIGHTER,
                -> 0.84f

                QualityPreset.WI_FI_GAMING,
                -> 1.00f

                QualityPreset.COMPETITIVE,
                -> 0.95f

                QualityPreset.INSTANT_PLAY,
                -> 1.0f

                QualityPreset.BALANCED_LOW_LATENCY -> 1.12f
                else -> 1.35f
            }

            StreamTransport.ADB_TUNNEL_TCP -> when (preset) {
                QualityPreset.TOURNAMENT_FIGHTER,
                QualityPreset.COMPETITIVE,
                -> 1.10f

                QualityPreset.WI_FI_GAMING -> 1.12f

                QualityPreset.INSTANT_PLAY -> 1.0f
                QualityPreset.BALANCED_LOW_LATENCY -> 1.2f
                else -> 1.35f
            }
        }
        return if (
            config.gamepadBoostEnabled &&
            config.transport == StreamTransport.UDP &&
            (preset == QualityPreset.TOURNAMENT_FIGHTER || preset == QualityPreset.WI_FI_GAMING)
        ) {
            baseFactor * 0.94f
        } else {
            baseFactor
        }
    }

    private data class CaptureSize(
        val width: Int,
        val height: Int,
    )

    private data class DisplayInfo(
        val width: Int,
        val height: Int,
        val densityDpi: Int,
        val rotation: Int,
    )

    private data class EncoderAttempt(
        val fps: Int,
        val bitrateBps: Int,
        val keyFrameIntervalSeconds: Int,
        val operatingRateFps: Float,
        val avcProfilePreference: AvcProfilePreference = AvcProfilePreference.DEFAULT,
    )

    private enum class AvcProfilePreference {
        DEFAULT,
        MAIN,
        BASELINE,
    }

    private data class ConfiguredEncoder(
        val encoder: MediaCodec,
        val inputSurface: Surface,
        val fps: Int,
        val bitrateBps: Int,
        val width: Int,
        val height: Int,
    )

    private data class PreparedVideoPayload(
        val payloadOffset: Int,
        val payloadSize: Int,
        val normalizedBytes: ByteArray?,
    )

    private data class PendingEnhancementMetadata(
        val x: Int,
        val y: Int,
        val width: Int,
        val height: Int,
        val screenWidth: Int,
        val screenHeight: Int,
        val pulseKind: String,
        val generation: Long,
    )

    private enum class VideoChannel {
        BASE,
        ENHANCEMENT,
    }
}
