package com.everty.receiver

import com.everty.receiver.audio.AudioPlayerStats
import com.everty.receiver.audio.PcmAudioPlayer
import com.everty.receiver.decoder.DecoderPreference
import com.everty.receiver.decoder.DecoderQueueStats
import com.everty.receiver.decoder.VideoDecoderWorker
import com.everty.receiver.transport.AudioConfig
import com.everty.receiver.transport.AudioFrameReassembler
import com.everty.receiver.transport.ControlMessage
import com.everty.receiver.transport.ControlPacketBuilder
import com.everty.receiver.transport.FrameReassembler
import com.everty.receiver.transport.SessionConfig
import com.everty.receiver.transport.TransportProtocol
import com.everty.receiver.transport.UdpPacket
import com.everty.receiver.transport.UdpReceiver
import java.net.InetSocketAddress
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicReference
import javax.swing.SwingUtilities
import kotlin.math.roundToInt
import org.bytedeco.javacv.Frame

class ReceiverController(
    private val onSnapshot: (ReceiverSnapshot) -> Unit,
    private val onFrame: (Frame) -> Unit,
    private val tryPresentDirect: (Frame) -> Boolean = { false },
) {
    private val lock = Any()
    private val pendingFrame = AtomicReference<Frame?>()
    private val frameDispatchScheduled = AtomicBoolean(false)
    private val pendingSnapshot = AtomicReference<ReceiverSnapshot?>()
    private val snapshotDispatchScheduled = AtomicBoolean(false)
    private val controlPacketBuilder = ControlPacketBuilder()
    private val frameDispatchExecutor = Executors.newSingleThreadExecutor { runnable ->
        Thread(runnable, "EvertyFrameDispatch").apply {
            isDaemon = true
        }
    }

    private var receiver: UdpReceiver? = null
    private var audioPlayer: PcmAudioPlayer? = null
    private var decoder: VideoDecoderWorker? = null
    private var reassembler: FrameReassembler? = null
    private var audioReassembler: AudioFrameReassembler? = null
    private var listening = false
    private var listeningPort = 0
    private var decoderPreference = DecoderPreference.AUTO
    private var ultraRealtime = false
    private var decoderSynced = false
    private var decoderCodecMime: String? = null
    private var senderEndpoint: InetSocketAddress? = null

    private var status = "Idle"
    private var sessionConfig: SessionConfig? = null
    private var sessionCodec = "-"
    private var decodePath = "-"
    private var audioStatus = "-"
    private var audioQueuedMs = 0
    private var audioDroppedChunks = 0L
    private var packetsReceived = 0L
    private var framesAssembled = 0L
    private var reassemblyDroppedFrames = 0L
    private var syncDroppedFrames = 0L
    private var presenterDroppedFrames = 0L
    private var framesDecoded = 0L
    private var decodeFps = 0
    private var decodedFramesInWindow = 0
    private var decodeWindowStartedAtNs = System.nanoTime()
    private var decoderBacklogFrames = 0
    private var decoderBacklogKb = 0
    private var decoderQueueDrops = 0L
    private var decoderWaitingForKeyFrame = false
    private var lastSnapshotPublishedAtNs = 0L
    private var lastFeedbackSentAtMs = 0L
    private var lastKeyFrameRequestSentAtMs = 0L
    private var lastFeedbackPressure = ControlMessage.PressureLevel.NORMAL
    private var lastFeedbackQueueDrops = 0L
    private var lastFeedbackPresenterDrops = 0L

    fun start(port: Int, decoderPreference: DecoderPreference, ultraRealtime: Boolean) {
        stop()

        val audioOutput = PcmAudioPlayer(
            onStatus = { message ->
                synchronized(lock) {
                    audioStatus = message
                }
                publishSnapshot(force = true)
            },
            onStats = { stats ->
                synchronized(lock) {
                    applyAudioStatsLocked(stats)
                }
                publishSnapshot()
            },
        )
        val pcmReassembler = AudioFrameReassembler(
            onAudioFrameReady = { bytes ->
                audioOutput.enqueue(bytes)
            },
        )

        val frameReassembler = FrameReassembler(
            onSessionConfig = { config ->
                synchronized(lock) {
                    sessionConfig = config
                    sessionCodec = codecLabel(config.codec)
                    decoderSynced = false
                    status = "Receiving $sessionCodec ${config.resolutionLabel}"
                }
                ensureDecoder(config.codec)
                publishSnapshot(force = true)
            },
            onKeyFrameReady = { bytes ->
                val decoderWorker = synchronized(lock) {
                    val currentDecoder = decoder
                    if (currentDecoder == null) {
                        syncDroppedFrames += 1
                        status = "Keyframe dropped because decoder is not ready"
                        null
                    } else {
                        decoderSynced = true
                        framesAssembled += 1
                        status = "Keyframe received"
                        currentDecoder
                    }
                }
                decoderWorker?.offerAccessUnit(bytes, isKeyFrame = true)
                publishSnapshot(force = true)
            },
            onInterFrameReady = { bytes ->
                val decoderWorker = synchronized(lock) {
                    val currentDecoder = decoder
                    if (decoderSynced && currentDecoder != null) {
                        framesAssembled += 1
                        currentDecoder
                    } else {
                        syncDroppedFrames += 1
                        null
                    }
                }
                decoderWorker?.offerAccessUnit(bytes, isKeyFrame = false)
                publishSnapshot()
            },
        )

        val udpReceiver = UdpReceiver(
            port = port,
            onPacket = { packet, remoteEndpoint ->
                handlePacket(packet, remoteEndpoint, frameReassembler)
            },
            onError = { message ->
                synchronized(lock) {
                    status = message
                }
                publishSnapshot(force = true)
            },
        )

        synchronized(lock) {
            audioPlayer = audioOutput
            reassembler = frameReassembler
            audioReassembler = pcmReassembler
            receiver = udpReceiver
            listening = true
            listeningPort = port
            this.decoderPreference = decoderPreference
            this.ultraRealtime = ultraRealtime
            decoderSynced = false
            decoderCodecMime = null
            senderEndpoint = null
            status = "Listening on UDP $port. Waiting for sender"
            sessionConfig = null
            sessionCodec = "-"
            decodePath = "-"
            audioStatus = "Waiting for audio"
            audioQueuedMs = 0
            audioDroppedChunks = 0
            packetsReceived = 0
            framesAssembled = 0
            reassemblyDroppedFrames = 0
            syncDroppedFrames = 0
            presenterDroppedFrames = 0
            framesDecoded = 0
            decodeFps = 0
            decodedFramesInWindow = 0
            decodeWindowStartedAtNs = System.nanoTime()
            decoderBacklogFrames = 0
            decoderBacklogKb = 0
            decoderQueueDrops = 0
            decoderWaitingForKeyFrame = false
            lastSnapshotPublishedAtNs = 0L
            lastFeedbackSentAtMs = 0L
            lastKeyFrameRequestSentAtMs = 0L
            lastFeedbackPressure = ControlMessage.PressureLevel.NORMAL
            lastFeedbackQueueDrops = 0L
            lastFeedbackPresenterDrops = 0L
        }

        udpReceiver.start()
        publishSnapshot(force = true)
    }

    fun updateDecoderPreference(preference: DecoderPreference) {
        val codecMimeType: String?
        synchronized(lock) {
            if (decoderPreference == preference) {
                return
            }
            decoderPreference = preference
            codecMimeType = decoderCodecMime
            status = "Decoder preference set to ${preference.uiLabel}"
        }
        publishSnapshot(force = true)
        if (codecMimeType != null) {
            ensureDecoder(codecMimeType, forceRestart = true)
            requestKeyFrame()
        }
    }

    fun updateUltraRealtime(enabled: Boolean) {
        val codecMimeType: String?
        synchronized(lock) {
            if (ultraRealtime == enabled) {
                return
            }
            ultraRealtime = enabled
            codecMimeType = decoderCodecMime
            status = if (enabled) {
                "Turbo realtime mode enabled"
            } else {
                "Turbo realtime mode disabled"
            }
        }
        publishSnapshot(force = true)
        if (codecMimeType != null) {
            ensureDecoder(codecMimeType, forceRestart = true)
            requestKeyFrame()
        }
    }

    fun stop() {
        val currentReceiver: UdpReceiver?
        val currentAudioPlayer: PcmAudioPlayer?
        val currentDecoder: VideoDecoderWorker?

        synchronized(lock) {
            currentReceiver = receiver
            currentAudioPlayer = audioPlayer
            currentDecoder = decoder
            receiver = null
            audioPlayer = null
            decoder = null
            reassembler = null
            audioReassembler = null
            listening = false
            listeningPort = 0
            decoderSynced = false
            decoderCodecMime = null
            senderEndpoint = null
            status = "Idle"
            sessionConfig = null
            sessionCodec = "-"
            decodePath = "-"
            audioStatus = "-"
            audioQueuedMs = 0
            audioDroppedChunks = 0
            packetsReceived = 0
            framesAssembled = 0
            reassemblyDroppedFrames = 0
            syncDroppedFrames = 0
            presenterDroppedFrames = 0
            framesDecoded = 0
            decodeFps = 0
            decodedFramesInWindow = 0
            decodeWindowStartedAtNs = System.nanoTime()
            decoderBacklogFrames = 0
            decoderBacklogKb = 0
            decoderQueueDrops = 0
            decoderWaitingForKeyFrame = false
            lastSnapshotPublishedAtNs = 0L
            lastFeedbackSentAtMs = 0L
            lastKeyFrameRequestSentAtMs = 0L
            lastFeedbackPressure = ControlMessage.PressureLevel.NORMAL
            lastFeedbackQueueDrops = 0L
            lastFeedbackPresenterDrops = 0L
        }

        runCatching { currentReceiver?.close() }
        runCatching { currentAudioPlayer?.close() }
        runCatching { currentDecoder?.close() }
        pendingFrame.getAndSet(null)?.let { frame ->
            runCatching { frame.close() }
        }
        publishSnapshot(force = true)
    }

    private fun ensureDecoder(codecMimeType: String, forceRestart: Boolean = false) {
        val previousDecoder: VideoDecoderWorker?
        val decoderPreferenceSnapshot: DecoderPreference
        val ultraRealtimeSnapshot: Boolean
        synchronized(lock) {
            if (!listening) {
                return
            }
            if (!forceRestart && decoder != null && decoderCodecMime.equals(codecMimeType, ignoreCase = true)) {
                return
            }

            previousDecoder = decoder
            decoderPreferenceSnapshot = decoderPreference
            ultraRealtimeSnapshot = ultraRealtime
            decoder = null
            decoderCodecMime = codecMimeType
            decoderSynced = false
            decodePath = "-"
            status = buildString {
                append("Starting ")
                append(codecLabel(codecMimeType))
                append(" decoder (")
                append(decoderPreferenceSnapshot.uiLabel)
                if (ultraRealtimeSnapshot) {
                    append(", Turbo")
                }
                append(")")
            }
        }

        runCatching { previousDecoder?.close() }

        val worker = VideoDecoderWorker(
            codecMimeType = codecMimeType,
            decoderPreference = decoderPreferenceSnapshot,
            ultraRealtime = ultraRealtimeSnapshot,
            onFrame = { frame ->
                synchronized(lock) {
                    framesDecoded += 1
                    decodedFramesInWindow += 1
                    updateDecodeFpsLocked()
                }
                dispatchFrame(frame)
                publishSnapshot()
            },
            onStatus = { message ->
                synchronized(lock) {
                    status = if (sessionConfig == null && listening) {
                        "Listening on UDP $listeningPort. $message"
                    } else {
                        message
                    }
                }
                publishSnapshot(force = true)
            },
            onError = { message ->
                synchronized(lock) {
                    status = message
                }
                publishSnapshot(force = true)
            },
            onQueueStats = { stats ->
                synchronized(lock) {
                    applyDecoderQueueStatsLocked(stats)
                    if (decoderWaitingForKeyFrame) {
                        status = "Decoder backlog dropped. Waiting for next keyframe"
                    }
                }
                maybeSendReceiverFeedback()
                publishSnapshot()
            },
            onDecodePathChanged = { path ->
                synchronized(lock) {
                    decodePath = path
                }
                publishSnapshot(force = true)
            },
        )

        synchronized(lock) {
            if (!listening || !decoderCodecMime.equals(codecMimeType, ignoreCase = true)) {
                runCatching { worker.close() }
                return
            }
            decoder = worker
        }

        worker.start()
        publishSnapshot(force = true)
    }

    private fun requestKeyFrame() {
        val receiverSnapshot: UdpReceiver
        val senderEndpointSnapshot: InetSocketAddress
        val requestBytes: ByteArray

        synchronized(lock) {
            receiverSnapshot = receiver ?: return
            senderEndpointSnapshot = senderEndpoint ?: return
            requestBytes = controlPacketBuilder.build(ControlMessage.RequestKeyFrame)
            lastKeyFrameRequestSentAtMs = System.currentTimeMillis()
        }

        runCatching {
            receiverSnapshot.send(requestBytes, senderEndpointSnapshot)
        }
    }

    private fun handlePacket(
        packet: UdpPacket,
        remoteEndpoint: InetSocketAddress,
        frameReassembler: FrameReassembler,
    ) {
        synchronized(lock) {
            packetsReceived += 1
            senderEndpoint = remoteEndpoint
        }
        when (packet.type) {
            TransportProtocol.TYPE_AUDIO_CONFIG -> {
                AudioConfig.parse(packet.payload)?.let { config ->
                    synchronized(lock) {
                        audioStatus = "${config.sampleRate} Hz / ${config.channels} ch"
                    }
                    audioPlayer?.configure(config)
                }
            }

            TransportProtocol.TYPE_AUDIO_FRAME -> {
                audioReassembler?.onPacket(packet)
                synchronized(lock) {
                    reassemblyDroppedFrames = frameReassembler.droppedFrames + (audioReassembler?.droppedFrames ?: 0L)
                }
            }

            else -> {
                frameReassembler.onPacket(packet)
                synchronized(lock) {
                    reassemblyDroppedFrames = frameReassembler.droppedFrames + (audioReassembler?.droppedFrames ?: 0L)
                }
            }
        }
        publishSnapshot()
    }

    private fun updateDecodeFpsLocked() {
        val now = System.nanoTime()
        val elapsedNs = now - decodeWindowStartedAtNs
        if (elapsedNs >= 1_000_000_000L) {
            decodeFps = ((decodedFramesInWindow * 1_000_000_000.0) / elapsedNs).roundToInt()
            decodedFramesInWindow = 0
            decodeWindowStartedAtNs = now
        }
    }

    private fun maybeSendReceiverFeedback() {
        val receiverSnapshot: UdpReceiver
        val senderEndpointSnapshot: InetSocketAddress
        val feedbackBytes: ByteArray
        var keyFrameBytes: ByteArray? = null
        var shouldRequestKeyFrame = false

        synchronized(lock) {
            receiverSnapshot = receiver ?: return
            senderEndpointSnapshot = senderEndpoint ?: return

            val now = System.currentTimeMillis()
            val targetFps = sessionConfig?.fps ?: 0
            val decodeFpsLow = targetFps >= 24 &&
                decodeFps > 0 &&
                decodeFps < if (ultraRealtime) {
                    maxOf(20, (targetFps * 0.85f).roundToInt())
                } else {
                    maxOf(18, (targetFps * 0.75f).roundToInt())
                }
            val highPressure = decoderWaitingForKeyFrame ||
                decoderBacklogFrames > 0 ||
                decoderQueueDrops > lastFeedbackQueueDrops ||
                presenterDroppedFrames > lastFeedbackPresenterDrops ||
                decodeFpsLow

            if (highPressure) {
                val feedbackIntervalMs = if (ultraRealtime) 150L else 250L
                if (now - lastFeedbackSentAtMs < feedbackIntervalMs) {
                    return
                }
                val keyFrameCooldownMs = if (ultraRealtime) 300L else 450L
                shouldRequestKeyFrame = now - lastKeyFrameRequestSentAtMs >= keyFrameCooldownMs
                feedbackBytes = controlPacketBuilder.build(
                    ControlMessage.ReceiverFeedback(
                        pressure = ControlMessage.PressureLevel.HIGH,
                        backlogFrames = decoderBacklogFrames,
                        queueDrops = decoderQueueDrops,
                        decodeFps = decodeFps,
                    ),
                )
                keyFrameBytes = if (shouldRequestKeyFrame) {
                    controlPacketBuilder.build(ControlMessage.RequestKeyFrame)
                } else {
                    null
                }
                lastFeedbackSentAtMs = now
                if (shouldRequestKeyFrame) {
                    lastKeyFrameRequestSentAtMs = now
                }
                lastFeedbackPressure = ControlMessage.PressureLevel.HIGH
                lastFeedbackQueueDrops = decoderQueueDrops
                lastFeedbackPresenterDrops = presenterDroppedFrames
            } else {
                val shouldSendNormal =
                    (lastFeedbackPressure == ControlMessage.PressureLevel.HIGH && now - lastFeedbackSentAtMs >= 700) ||
                        now - lastFeedbackSentAtMs >= 1_500
                if (!shouldSendNormal) {
                    return
                }
                feedbackBytes = controlPacketBuilder.build(
                    ControlMessage.ReceiverFeedback(
                        pressure = ControlMessage.PressureLevel.NORMAL,
                        backlogFrames = decoderBacklogFrames,
                        queueDrops = decoderQueueDrops,
                        decodeFps = decodeFps,
                    ),
                )
                lastFeedbackSentAtMs = now
                lastFeedbackPressure = ControlMessage.PressureLevel.NORMAL
                lastFeedbackQueueDrops = decoderQueueDrops
                lastFeedbackPresenterDrops = presenterDroppedFrames
            }
        }

        runCatching {
            receiverSnapshot.send(feedbackBytes, senderEndpointSnapshot)
            if (keyFrameBytes != null) {
                receiverSnapshot.send(keyFrameBytes!!, senderEndpointSnapshot)
            }
        }
    }

    private fun publishSnapshot(force: Boolean = false) {
        val snapshot = synchronized(lock) {
            val now = System.nanoTime()
            if (!force && now - lastSnapshotPublishedAtNs < 150_000_000L) {
                null
            } else {
                lastSnapshotPublishedAtNs = now
                ReceiverSnapshot(
                    listening = listening,
                    status = status,
                    sessionCodec = sessionCodec,
                    decodePath = decodePath,
                    audioStatus = audioStatus,
                    audioQueuedMs = audioQueuedMs,
                    audioDroppedChunks = audioDroppedChunks,
                    sessionPreset = sessionConfig?.preset ?: "-",
                    resolution = sessionConfig?.resolutionLabel ?: "-",
                    fpsTarget = sessionConfig?.fps ?: 0,
                    bitrateMbps = ((sessionConfig?.bitrate ?: 0) / 1_000_000.0),
                    packetsReceived = packetsReceived,
                    framesAssembled = framesAssembled,
                    framesDropped = reassemblyDroppedFrames + syncDroppedFrames + decoderQueueDrops + presenterDroppedFrames,
                    framesDecoded = framesDecoded,
                    decodeFps = decodeFps,
                    decoderBacklogFrames = decoderBacklogFrames,
                    decoderBacklogKb = decoderBacklogKb,
                    decoderQueueDrops = decoderQueueDrops,
                    decoderWaitingForKeyFrame = decoderWaitingForKeyFrame,
                )
            }
        } ?: return

        pendingSnapshot.set(snapshot)
        scheduleSnapshotDispatch()
    }

    private fun applyDecoderQueueStatsLocked(stats: DecoderQueueStats) {
        decoderBacklogFrames = stats.queuedUnits
        decoderBacklogKb = stats.queuedBytes / 1024
        decoderQueueDrops = stats.droppedUnits
        decoderWaitingForKeyFrame = stats.waitingForKeyFrame
    }

    private fun applyAudioStatsLocked(stats: AudioPlayerStats) {
        audioQueuedMs = stats.queuedMs
        audioDroppedChunks = stats.droppedChunks
    }

    private fun dispatchFrame(frame: Frame) {
        val presentDirect = synchronized(lock) { ultraRealtime }
        if (presentDirect && runCatching { tryPresentDirect(frame) }.getOrDefault(false)) {
            return
        }

        val clonedFrame = runCatching { frame.clone() }.getOrNull() ?: return
        val previousFrame = pendingFrame.getAndSet(clonedFrame)
        if (previousFrame != null) {
            synchronized(lock) {
                presenterDroppedFrames += 1
            }
            runCatching { previousFrame.close() }
            maybeSendReceiverFeedback()
        }
        scheduleFrameDispatch()
    }

    private fun scheduleSnapshotDispatch() {
        if (!snapshotDispatchScheduled.compareAndSet(false, true)) {
            return
        }

        SwingUtilities.invokeLater {
            val latest = pendingSnapshot.getAndSet(null)
            if (latest != null) {
                onSnapshot(latest)
            }
            snapshotDispatchScheduled.set(false)
            if (pendingSnapshot.get() != null) {
                scheduleSnapshotDispatch()
            }
        }
    }

    private fun scheduleFrameDispatch() {
        if (!frameDispatchScheduled.compareAndSet(false, true)) {
            return
        }

        frameDispatchExecutor.execute {
            try {
                val latest = pendingFrame.getAndSet(null)
                if (latest != null) {
                    try {
                        onFrame(latest)
                    } catch (t: Throwable) {
                        synchronized(lock) {
                            presenterDroppedFrames += 1
                            status = "Frame presenter error: ${t.message ?: t.javaClass.simpleName}"
                        }
                        publishSnapshot(force = true)
                    } finally {
                        runCatching { latest.close() }
                    }
                }
            } finally {
                frameDispatchScheduled.set(false)
                if (pendingFrame.get() != null) {
                    scheduleFrameDispatch()
                }
            }
        }
    }

    private fun codecLabel(codecMimeType: String): String {
        return when {
            codecMimeType.equals("video/hevc", ignoreCase = true) -> "H.265 / HEVC"
            else -> "H.264 / AVC"
        }
    }
}
