package com.everty.evertygame.receiver

import android.content.Context
import android.content.pm.PackageManager
import android.media.MediaCodec
import android.media.MediaCodecList
import android.media.MediaFormat
import android.app.UiModeManager
import android.content.res.Configuration
import android.os.Handler
import android.os.HandlerThread
import android.os.Looper
import android.os.Process
import android.util.Log
import android.view.InputDevice
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.Surface
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.everty.evertygame.stream.TransportProtocol
import java.io.IOException
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.Inet4Address
import java.net.InetSocketAddress
import java.net.NetworkInterface
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.util.concurrent.atomic.AtomicReference
import kotlin.math.max
import kotlin.math.min
import kotlinx.coroutines.delay

enum class PcReceiverPhase {
    IDLE,
    LISTENING,
    REGISTERING_RELAY,
    WAITING_FIRST_PACKET,
    STOPPING,
    CONNECTED,
    ERROR,
}

data class PcReceiverMetrics(
    val packetsReceived: Long = 0,
    val audioPacketsReceived: Long = 0,
    val framesDecoded: Long = 0,
    val framesDropped: Long = 0,
    val decodeFps: Int = 0,
    val pulseToAndroidEstimateMs: Int = -1,
    val inputToAndroidEstimateMs: Int = -1,
    val resolutionLabel: String = "-",
    val codecLabel: String = "-",
    val remoteEndpoint: String = "-",
    val videoWidth: Int = 0,
    val videoHeight: Int = 0,
    val audioStatus: String = "-",
    val decoderPath: String = "-",
    val decoderTuning: String = "-",
    val lastGamepadKey: String = "-",
    val lastGamepadMotion: String = "-",
)

data class PcReceiverUiState(
    val phase: PcReceiverPhase = PcReceiverPhase.IDLE,
    val status: String = "Р“РѕС‚РѕРІ Рє РїРѕРґРєР»СЋС‡РµРЅРёСЋ",
    val listenPort: Int = 5001,
    val localAddressHint: String = "-",
    val touchControlEnabled: Boolean = false,
    val metrics: PcReceiverMetrics = PcReceiverMetrics(),
    val lastError: String? = null,
)

data class PcReceiverDiagnostics(
    val sessionId: String = "-",
    val routeKind: String = "-",
    val relayEndpoint: String = "-",
    val receiverRegistered: Boolean = false,
    val lastRelayAck: String = "-",
    val firstPacketReceived: Boolean = false,
    val lastControlSendError: String = "-",
    val lastMediaPacketFrom: String = "-",
)

internal class PcReceiverClientController(
    private val context: Context,
) {
    private data class RelayRoute(
        val sessionId: String,
        val sessionToken: String,
        val endpoint: InetSocketAddress,
    )

    private data class RemoteGamepadState(
        val controllerId: Int = 0,
        val keyButtons: Int = 0,
        val motionButtons: Int = 0,
        val leftTrigger: Int = 0,
        val rightTrigger: Int = 0,
        val leftThumbX: Int = 0,
        val leftThumbY: Int = 0,
        val rightThumbX: Int = 0,
        val rightThumbY: Int = 0,
    ) {
        val buttons: Int
            get() = keyButtons or motionButtons
    }

    private data class ControllerBindingKey(
        val descriptor: String,
        val vendorId: Int,
        val productId: Int,
        val name: String,
    )

    companion object {
        private val activeControllerRef = AtomicReference<PcReceiverClientController?>()
        private const val XINPUT_GAMEPAD_DPAD_UP = 0x0001
        private const val XINPUT_GAMEPAD_DPAD_DOWN = 0x0002
        private const val XINPUT_GAMEPAD_DPAD_LEFT = 0x0004
        private const val XINPUT_GAMEPAD_DPAD_RIGHT = 0x0008
        private const val XINPUT_GAMEPAD_START = 0x0010
        private const val XINPUT_GAMEPAD_BACK = 0x0020
        private const val XINPUT_GAMEPAD_LEFT_THUMB = 0x0040
        private const val XINPUT_GAMEPAD_RIGHT_THUMB = 0x0080
        private const val XINPUT_GAMEPAD_LEFT_SHOULDER = 0x0100
        private const val XINPUT_GAMEPAD_RIGHT_SHOULDER = 0x0200
        private const val XINPUT_GAMEPAD_A = 0x1000
        private const val XINPUT_GAMEPAD_B = 0x2000
        private const val XINPUT_GAMEPAD_X = 0x4000
        private const val XINPUT_GAMEPAD_Y = 0x8000

        fun setActiveController(controller: PcReceiverClientController?) {
            activeControllerRef.set(controller)
        }

        fun activeController(): PcReceiverClientController? = activeControllerRef.get()
    }

    private val mainHandler = Handler(Looper.getMainLooper())
    private val sync = Any()
    private val decodeTicks = ArrayDeque<Long>()
    private val recentPulseToAndroidEstimates = ArrayDeque<Int>()
    private val recentInputToAndroidEstimates = ArrayDeque<Int>()
    private val frameArrivalNsByPts = LinkedHashMap<Long, Long>()
    private val framePresentedNsByPts = LinkedHashMap<Long, Long>()
    private val pendingLatencyPulses = LinkedHashMap<Long, PendingLatencyPulse>()
    private val latencyRequestStartedAtNsBySeq = LinkedHashMap<Long, Long>()
    private val frameAssembler = AccessUnitAssembler(::onAccessUnitReady, ::onFrameDropped)
    private val audioFrameAssembler = PcReceiverAudioFrameReassembler(::onAudioFrameReady)
    private val audioPlaybackSink = PcReceiverAudioPlaybackSink()

    private var socket: DatagramSocket? = null
    private var receiveThread: Thread? = null
    private var running = false
    private var surface: Surface? = null
    private var decoder: MediaCodec? = null
    private var nativeDecoder: NativeVideoDecoderBridge? = null
    private var decoderConfig: DecoderConfig? = null
    private var blockedDecoderConfig: DecoderConfig? = null
    private var blockedDecoderMessage: String? = null
    private var decoderConfigureInFlight = false
    private var pendingDecoderReconfigure = false
    private var pendingDecoderReconfigureForce = false
    private var sessionConfig: SessionConfig? = null
    private var codecConfig: ByteArray? = null
    private var pendingSessionConfig: SessionConfig? = null
    private var lastSessionConfigAppliedAtNs = 0L
    private var remoteEndpoint: InetSocketAddress? = null
    private var inputSeq = 0L
    private var waitingForKeyFrame = true
    private var lastMouseButtonState = 0
    private var framesDroppedCounter = 0L
    private var lastPacketReceivedAtNs = 0L
    private var lastFeedbackSentAtNs = 0L
    private var lastFeedbackDropCount = 0L
    private var lastLatencyRequestSentAtNs = 0L
    private var pulseToAndroidEstimateMs = -1
    private var inputToAndroidEstimateMs = -1
    private var lastQueuedPresentationTimeUs = Long.MIN_VALUE
    private var controlSendThread: HandlerThread? = null
    private var controlSendHandler: Handler? = null
    private var pendingAbsoluteMovePayload: ByteArray? = null
    private var absoluteMoveDispatchQueued = false
    private var relayRoute: RelayRoute? = null
    private var relayRegistrationRoute: RelayRoute? = null
    private var relayRegisterRunnable: Runnable? = null
    private var preferNativeDecoder = true
    private val gamepadStates = linkedMapOf<Int, RemoteGamepadState>()
    private val lastSentGamepadStates = linkedMapOf<Int, RemoteGamepadState>()
    private val controllerSlotByDeviceId = linkedMapOf<Int, Int>()
    private val controllerSlotByBindingKey = linkedMapOf<ControllerBindingKey, Int>()
    private var lastActiveControllerId = 0
    private var syntheticCircleReleaseToken = 0L
    private var sawStandardFaceButtonA = false
    private var sawStandardFaceButtons = false
    private var sawStandardFaceButtonB = false
    private var loggedFirstRawUdpPacket = false
    private var loggedFirstSessionConfigPacket = false
    private var loggedFirstCodecConfigPacket = false
    private var loggedFirstVideoPacket = false
    private var loggedFirstAudioPacket = false
    private var loggedFirstDecodedFrame = false
    @Volatile
    private var receiverFeedbackListener: ((ReceiverFeedbackSnapshot) -> Unit)? = null
    @Volatile
    private var eventListener: ((String) -> Unit)? = null

    var uiState by mutableStateOf(
        PcReceiverUiState(
            localAddressHint = buildLocalAddressHint(5001),
        ),
    )
        private set

    var diagnostics by mutableStateOf(PcReceiverDiagnostics())
        private set

    fun start(listenPort: Int) {
        val keepSocket = synchronized(sync) {
            val current = socket
            running && current != null && !current.isClosed && current.localPort == listenPort
        }

        if (keepSocket) {
            stop(closeSocket = false)
        } else {
            stop(closeSocket = true)
        }

        ensureControlSenderThread()

        val localHint = buildLocalAddressHint(listenPort)
        val datagramSocket = synchronized(sync) { socket } ?: DatagramSocket(listenPort).apply {
            reuseAddress = true
            receiveBufferSize = 512 * 1024
            soTimeout = 1000
        }

        synchronized(sync) {
            socket = datagramSocket
            running = true
            remoteEndpoint = null
            sessionConfig = null
            codecConfig = null
            pendingSessionConfig = null
            decoderConfig = null
            blockedDecoderConfig = null
            blockedDecoderMessage = null
            decoderConfigureInFlight = false
            pendingDecoderReconfigure = false
            pendingDecoderReconfigureForce = false
            waitingForKeyFrame = true
            lastMouseButtonState = 0
            framesDroppedCounter = 0L
            lastPacketReceivedAtNs = 0L
            lastFeedbackSentAtNs = 0L
            lastFeedbackDropCount = 0L
            lastLatencyRequestSentAtNs = 0L
            pulseToAndroidEstimateMs = -1
            inputToAndroidEstimateMs = -1
            lastSessionConfigAppliedAtNs = 0L
            lastQueuedPresentationTimeUs = Long.MIN_VALUE
            frameAssembler.reset()
            audioFrameAssembler.reset()
            decodeTicks.clear()
            recentPulseToAndroidEstimates.clear()
            recentInputToAndroidEstimates.clear()
            frameArrivalNsByPts.clear()
            framePresentedNsByPts.clear()
            pendingLatencyPulses.clear()
            latencyRequestStartedAtNsBySeq.clear()
            gamepadStates.clear()
            lastSentGamepadStates.clear()
            controllerSlotByDeviceId.clear()
            controllerSlotByBindingKey.clear()
            lastActiveControllerId = 0
            syntheticCircleReleaseToken = 0L
            sawStandardFaceButtonA = false
            sawStandardFaceButtons = false
            sawStandardFaceButtonB = false
            loggedFirstRawUdpPacket = false
            loggedFirstSessionConfigPacket = false
            loggedFirstCodecConfigPacket = false
            loggedFirstVideoPacket = false
            loggedFirstAudioPacket = false
            loggedFirstDecodedFrame = false
        }

        mutateDiagnostics {
            PcReceiverDiagnostics()
        }
        mutateState {
            it.copy(
                phase = PcReceiverPhase.LISTENING,
                status = "РџРѕРґРєР»СЋС‡РµРЅРёРµ Рє РџРљ...",
                listenPort = listenPort,
                localAddressHint = localHint,
                touchControlEnabled = true,
                metrics = PcReceiverMetrics(remoteEndpoint = "-"),
                lastError = null,
            )
        }

        val (currentSocket, currentThread) = synchronized(sync) { Pair(socket, receiveThread) }
        val threadDead = currentThread == null || !currentThread.isAlive
        if (currentSocket == null || currentSocket != datagramSocket || threadDead) {
            receiveThread = Thread(
                {
                    receiveLoop(datagramSocket)
                },
                "EvertyPcReceiverUdp",
            ).apply {
                isDaemon = true
                start()
            }
        }

        updateRelayRegistrationLoop()
    }

    fun stop(closeSocket: Boolean = true) {
        val threadToJoin: Thread?
        val socketToClose: DatagramSocket?
        val sendThreadToStop: HandlerThread?
        val relayRegisterToRemove: Runnable?
        val handlerToClear: Handler?
        synchronized(sync) {
            if (closeSocket) {
                running = false
                threadToJoin = receiveThread
                socketToClose = socket
                receiveThread = null
                socket = null
            } else {
                threadToJoin = null
                socketToClose = null
            }
            sendThreadToStop = controlSendThread
            relayRegisterToRemove = relayRegisterRunnable
            handlerToClear = controlSendHandler

            remoteEndpoint = null
            controlSendThread = null
            controlSendHandler = null
            pendingAbsoluteMovePayload = null
            absoluteMoveDispatchQueued = false
            relayRoute = null
            relayRegistrationRoute = null
            relayRegisterRunnable = null
            gamepadStates.clear()
            lastSentGamepadStates.clear()
            controllerSlotByDeviceId.clear()
            controllerSlotByBindingKey.clear()
            lastActiveControllerId = 0
            syntheticCircleReleaseToken = 0L
            sawStandardFaceButtonA = false
            sawStandardFaceButtons = false
            sawStandardFaceButtonB = false
            blockedDecoderConfig = null
            blockedDecoderMessage = null
            waitingForKeyFrame = true
            lastMouseButtonState = 0
            framesDroppedCounter = 0L
            lastPacketReceivedAtNs = 0L
            lastFeedbackSentAtNs = 0L
            lastFeedbackDropCount = 0L
            lastLatencyRequestSentAtNs = 0L
            pulseToAndroidEstimateMs = -1
            inputToAndroidEstimateMs = -1
            lastQueuedPresentationTimeUs = Long.MIN_VALUE
            frameAssembler.reset()
            audioFrameAssembler.reset()
            recentPulseToAndroidEstimates.clear()
            recentInputToAndroidEstimates.clear()
            frameArrivalNsByPts.clear()
            framePresentedNsByPts.clear()
            pendingLatencyPulses.clear()
            latencyRequestStartedAtNsBySeq.clear()
        }

        if (handlerToClear != null && relayRegisterToRemove != null) {
            handlerToClear.removeCallbacks(relayRegisterToRemove)
        }
        sendReleaseAll()
        socketToClose?.close()
        sendThreadToStop?.quitSafely()
        threadToJoin?.join(300)
        releaseDecoder()
        audioPlaybackSink.release()

        mutateDiagnostics {
            PcReceiverDiagnostics()
        }
        mutateState {
            PcReceiverUiState(
                phase = PcReceiverPhase.IDLE,
                status = "РЎРµСЃСЃРёСЏ РѕСЃС‚Р°РЅРѕРІР»РµРЅР°",
                listenPort = it.listenPort,
                localAddressHint = buildLocalAddressHint(it.listenPort),
                touchControlEnabled = false,
            )
        }
    }

    fun requestLatencyMeasurement() {
        if (uiState.phase != PcReceiverPhase.CONNECTED) {
            return
        }

        requestLatencyMeasurementInternal("android_manual", force = true)
    }

    fun resolveReceiverEndpoint(listenPort: Int = uiState.listenPort): Pair<String, Int>? {
        val host = findLocalIpv4Address() ?: return null
        return host to listenPort
    }

    fun configureManagedRelayRoute(
        sessionId: String?,
        sessionToken: String?,
        relayHost: String?,
        relayPort: Int?,
    ) {
        val route: RelayRoute?
        val currentSocket: DatagramSocket?
        val routeChanged: Boolean
        synchronized(sync) {
            route = if (!sessionId.isNullOrBlank() && !sessionToken.isNullOrBlank() && !relayHost.isNullOrBlank() && relayPort != null && relayPort in 1..65535) {
                RelayRoute(
                    sessionId = sessionId.trim(),
                    sessionToken = sessionToken.trim(),
                    endpoint = InetSocketAddress(relayHost.trim(), relayPort),
                )
            } else {
                null
            }
            val previousRoute = relayRoute
            routeChanged =
                previousRoute?.sessionId != route?.sessionId ||
                previousRoute?.sessionToken != route?.sessionToken ||
                previousRoute?.endpoint?.hostString != route?.endpoint?.hostString ||
                previousRoute?.endpoint?.port != route?.endpoint?.port
            relayRoute = route
            currentSocket = socket
        }

        if (currentSocket != null) {
            // Unconditionally calling disconnect() can brick DatagramSocket
            // on some Android kernels regarding incoming packets.
        }
        if (routeChanged) {
            mutateDiagnostics { current ->
                current.copy(
                    sessionId = sessionId?.trim().orEmpty().ifBlank { current.sessionId },
                    relayEndpoint = route?.endpoint?.let { "${it.hostString}:${it.port}" } ?: current.relayEndpoint,
                    firstPacketReceived = false,
                    lastMediaPacketFrom = "-",
                    lastControlSendError = "-",
                )
            }
        }
        updateRelayRegistrationLoop()
    }

    fun configureManagedRelayRegistration(
        sessionId: String?,
        sessionToken: String?,
        relayHost: String?,
        relayPort: Int?,
    ) {
        val route =
            if (!sessionId.isNullOrBlank() && !sessionToken.isNullOrBlank() && !relayHost.isNullOrBlank() && relayPort != null && relayPort in 1..65535) {
                RelayRoute(
                    sessionId = sessionId.trim(),
                    sessionToken = sessionToken.trim(),
                    endpoint = InetSocketAddress(relayHost.trim(), relayPort),
                )
            } else {
                null
            }

        val routeChanged = synchronized(sync) {
            val previousRoute = relayRegistrationRoute
            val changed =
                previousRoute?.sessionId != route?.sessionId ||
                previousRoute?.sessionToken != route?.sessionToken ||
                previousRoute?.endpoint?.hostString != route?.endpoint?.hostString ||
                previousRoute?.endpoint?.port != route?.endpoint?.port
            relayRegistrationRoute = route
            changed
        }

        if (routeChanged) {
            mutateDiagnostics { current ->
                current.copy(
                    sessionId = sessionId?.trim().orEmpty().ifBlank { current.sessionId },
                    relayEndpoint = route?.endpoint?.let { "${it.hostString}:${it.port}" } ?: "-",
                    receiverRegistered = false,
                    lastRelayAck = "-",
                )
            }
        }

        updateRelayRegistrationLoop()
    }

    fun updateManagedDiagnostics(sessionId: String, routeKind: String, relayHost: String?, relayPort: Int?) {
        mutateDiagnostics { current ->
            val relayEndpoint = relayHost?.takeIf { it.isNotBlank() }?.let { host ->
                "${host.trim()}:${relayPort?.takeIf { it in 1..65535 } ?: 0}"
            } ?: current.relayEndpoint
            val changed =
                current.sessionId != sessionId ||
                current.routeKind != routeKind.ifBlank { "-" } ||
                current.relayEndpoint != relayEndpoint
            current.copy(
                sessionId = sessionId,
                routeKind = routeKind.ifBlank { "-" },
                relayEndpoint = relayEndpoint,
                receiverRegistered = if (changed) false else current.receiverRegistered,
                lastRelayAck = if (changed) "-" else current.lastRelayAck,
                firstPacketReceived = if (changed) false else current.firstPacketReceived,
                lastMediaPacketFrom = if (changed) "-" else current.lastMediaPacketFrom,
                lastControlSendError = if (changed) "-" else current.lastControlSendError,
            )
        }
    }

    fun markRegisteringRelay() {
        mutateDiagnostics { current ->
            current.copy(
                receiverRegistered = false,
                lastRelayAck = "-",
                firstPacketReceived = false,
                lastMediaPacketFrom = "-",
                lastControlSendError = "-",
            )
        }
        mutateState { current ->
            current.copy(
                phase = PcReceiverPhase.REGISTERING_RELAY,
                status = "Р РµРіРёСЃС‚СЂР°С†РёСЏ С‚РµР»РµС„РѕРЅР° РІ relay...",
                lastError = null,
            )
        }
    }

    fun markWaitingForFirstPacket() {
        mutateState { current ->
            current.copy(
                phase = PcReceiverPhase.WAITING_FIRST_PACKET,
                status = "РћР¶РёРґР°РЅРёРµ РїРµСЂРІРѕРіРѕ РєР°РґСЂР°...",
                lastError = null,
            )
        }
    }

    fun markStopping() {
        mutateState { current ->
            current.copy(
                phase = PcReceiverPhase.STOPPING,
                status = "РћСЃС‚Р°РЅРѕРІРєР° СЃРµСЃСЃРёРё...",
            )
        }
    }

    suspend fun awaitRelayRegistrationAck(sessionId: String, timeoutMs: Long): Boolean {
        val deadline = System.nanoTime() + timeoutMs * 1_000_000L
        while (System.nanoTime() < deadline) {
            val current = diagnostics
            if (current.sessionId == sessionId && current.receiverRegistered) {
                return true
            }
            delay(100L)
        }
        val current = diagnostics
        return current.sessionId == sessionId && current.receiverRegistered
    }

    suspend fun awaitFirstPacket(timeoutMs: Long): Boolean {
        val deadline = System.nanoTime() + timeoutMs * 1_000_000L
        while (System.nanoTime() < deadline) {
            if (diagnostics.firstPacketReceived) {
                return true
            }
            delay(100L)
        }
        return diagnostics.firstPacketReceived
    }

    fun sendControlPing(reason: String = "connect") {
        Log.d("EVRT", "Sending control ping: $reason")
        enqueueControl(buildReceiverPing(reason))
    }

    fun requestKeyFrame(reason: String = "connect") {
        Log.d("EVRT", "Requesting key frame: $reason")
        enqueueControl(buildRequestKeyFrame())
    }

    fun sendReceiverStop(reason: String = "manual_stop") {
        Log.d("EVRT", "Sending receiver stop: $reason")
        eventListener?.invoke("RECEIVER STOP SEND | reason=$reason")
        enqueueControl(buildReceiverStop(reason))
    }

    fun sendRelayRegistrationNow() {
        val route = synchronized(sync) { relayRegistrationRoute }
        if (route == null) {
            Log.d("EVRT", "Skipping relay registration: route not ready.")
            eventListener?.invoke("RELAY REGISTER SKIP | route=missing")
            return
        }
        Log.d("EVRT", "Sending relay registration for session ${route.sessionId} to ${route.endpoint.hostString}:${route.endpoint.port}")
        eventListener?.invoke("RELAY REGISTER SEND | session=${route.sessionId} | target=${route.endpoint.hostString}:${route.endpoint.port}")
        enqueueControl(buildRelayRegistration(route.sessionId, route.sessionToken, "receiver"))
    }

    fun armRelayRegistrationBurst(reason: String = "connect") {
        val route = synchronized(sync) { relayRegistrationRoute }
        if (route == null) {
            Log.d("EVRT", "Skipping relay registration burst: route not ready.")
            eventListener?.invoke("RELAY REGISTER BURST SKIP | route=missing | reason=$reason")
            return
        }

        Log.d("EVRT", "Arming relay registration burst for ${route.sessionId}; reason=$reason")
        eventListener?.invoke("RELAY REGISTER BURST | session=${route.sessionId} | target=${route.endpoint.hostString}:${route.endpoint.port} | reason=$reason")
        sendRelayRegistrationNow()
        mainHandler.postDelayed({ sendRelayRegistrationNow() }, 350L)
        mainHandler.postDelayed({ sendRelayRegistrationNow() }, 900L)
        mainHandler.postDelayed({ sendRelayRegistrationNow() }, 1800L)
    }

    fun attachSurface(surface: Surface?) {
        synchronized(sync) {
            this.surface = surface
        }
        maybeConfigureDecoder()
    }

    fun setTouchControlEnabled(enabled: Boolean) {
        mutateState { current ->
            current.copy(touchControlEnabled = enabled)
        }
        synchronized(sync) {
            if (!enabled) {
                latencyRequestStartedAtNsBySeq.clear()
                recentInputToAndroidEstimates.clear()
                inputToAndroidEstimateMs = -1
                gamepadStates.clear()
                lastSentGamepadStates.clear()
                controllerSlotByDeviceId.clear()
                controllerSlotByBindingKey.clear()
                lastActiveControllerId = 0
                syntheticCircleReleaseToken = 0L
                sawStandardFaceButtonA = false
                sawStandardFaceButtons = false
                sawStandardFaceButtonB = false
            }
        }
        if (!enabled) {
            lastMouseButtonState = 0
            sendReleaseAll()
        }
    }

    fun handleSystemBackAsGamepad(): Boolean {
        val state = uiState
        if (!state.touchControlEnabled || state.phase != PcReceiverPhase.CONNECTED) {
            return false
        }

        mutateState { current ->
            current.copy(
                metrics = current.metrics.copy(
                    lastGamepadKey = "system BACK -> Circle/B",
                ),
            )
        }

        val controllerId = synchronized(sync) { lastActiveControllerId }
        val releaseToken = synchronized(sync) {
            syntheticCircleReleaseToken += 1
            val current = getOrCreateGamepadStateLocked(controllerId)
            gamepadStates[controllerId] = current.copy(keyButtons = current.keyButtons or XINPUT_GAMEPAD_B)
            syntheticCircleReleaseToken
        }
        emitGamepadStateIfChanged(controllerId)

        mainHandler.postDelayed(
            {
                val shouldRelease = synchronized(sync) {
                    if (syntheticCircleReleaseToken != releaseToken) {
                        false
                    } else {
                        val current = getOrCreateGamepadStateLocked(controllerId)
                        gamepadStates[controllerId] = current.copy(keyButtons = current.keyButtons and XINPUT_GAMEPAD_B.inv())
                        true
                    }
                }
                if (shouldRelease) {
                    emitGamepadStateIfChanged(controllerId)
                }
            },
            45L,
        )
        return true
    }

    fun handlePointerEvent(event: MotionEvent, viewWidth: Int, viewHeight: Int): Boolean {
        val state = uiState
        if (state.phase != PcReceiverPhase.CONNECTED) {
            return false
        }

        if (isGamepadMotionEvent(event)) {
            if (event.actionMasked != MotionEvent.ACTION_CANCEL) {
                updateGamepadAxes(event)
                return true
            }
            return false
        }

        if (!state.touchControlEnabled || viewWidth <= 0 || viewHeight <= 0) {
            return false
        }

        val config = synchronized(sync) { sessionConfig } ?: return false
        val mapped = mapTouchToNormalized(
            event.x,
            event.y,
            viewWidth,
            viewHeight,
            config.width,
            config.height,
        )

        enqueueAbsoluteMouseMove(buildRemoteMouseMoveAbsolute(nextInputSeq(), mapped.first, mapped.second))

        if (event.isFromSource(InputDevice.SOURCE_MOUSE)) {
            syncMouseButtons(event.buttonState)
            if (event.actionMasked == MotionEvent.ACTION_SCROLL) {
                val wheelDelta = (-event.getAxisValue(MotionEvent.AXIS_VSCROLL) * 120f).toInt()
                if (wheelDelta != 0) {
                    enqueueControl(buildRemoteMouseWheel(nextInputSeq(), wheelDelta))
                }
            }
            return true
        }

        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                enqueueControl(buildRemoteMouseButton(nextInputSeq(), "left", pressed = true))
            }

            MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> {
                enqueueControl(buildRemoteMouseButton(nextInputSeq(), "left", pressed = false))
            }
        }

        return true
    }

    fun handleKeyEvent(event: KeyEvent): Boolean {
        val state = uiState
        if (!state.touchControlEnabled || state.phase != PcReceiverPhase.CONNECTED) {
            return false
        }

        if (isGamepadLikeEvent(event)) {
            return handleGamepadKeyEvent(event)
        }

        if (event.action != KeyEvent.ACTION_DOWN && event.action != KeyEvent.ACTION_UP) {
            return false
        }

        if (event.action == KeyEvent.ACTION_DOWN && event.repeatCount > 0) {
            return true
        }

        if (event.keyCode == KeyEvent.KEYCODE_BACK) {
            if (event.isFromSource(InputDevice.SOURCE_MOUSE)) {
                enqueueControl(buildRemoteMouseButton(nextInputSeq(), "right", pressed = event.action == KeyEvent.ACTION_DOWN))
            } else {
                enqueueControl(buildRemoteKey(nextInputSeq(), 0x1B, pressed = event.action == KeyEvent.ACTION_DOWN))
            }
            return true
        }

        if (event.keyCode == KeyEvent.KEYCODE_HOME) {
            return false
        }

        val virtualKey = mapAndroidKeyCodeToWindowsVKey(event.keyCode) ?: return false
        enqueueControl(buildRemoteKey(nextInputSeq(), virtualKey, pressed = event.action == KeyEvent.ACTION_DOWN))
        return true
    }

    fun setPreferNativeDecoder(prefer: Boolean) {
        val changed = synchronized(sync) {
            if (preferNativeDecoder == prefer) {
                false
            } else {
                preferNativeDecoder = prefer
                true
            }
        }
        if (changed && uiState.phase == PcReceiverPhase.CONNECTED) {
            mainHandler.post {
                maybeConfigureDecoder(force = true)
            }
        }
    }

    fun noteActivityKeyEvent(event: KeyEvent) {
        noteCapturedKeyEvent("activity", event)
    }

    fun noteAccessibilityKeyEvent(event: KeyEvent) {
        noteCapturedKeyEvent("accessibility", event)
    }

    private fun noteCapturedKeyEvent(channel: String, event: KeyEvent) {
        val deviceName = event.device?.name ?: "unknown"
        val sources = event.device?.sources ?: event.source
        mutateState { current ->
            current.copy(
                metrics = current.metrics.copy(
                    lastGamepadKey = "$channel key code=${event.keyCode} action=${event.action} src=0x${sources.toString(16)} $deviceName",
                ),
            )
        }
    }

    fun noteActivityMotionEvent(event: MotionEvent) {
        val deviceName = event.device?.name ?: "unknown"
        val sources = event.device?.sources ?: event.source
        val x = runCatching { event.getAxisValue(MotionEvent.AXIS_X) }.getOrDefault(0f)
        val y = runCatching { event.getAxisValue(MotionEvent.AXIS_Y) }.getOrDefault(0f)
        val hatX = runCatching { event.getAxisValue(MotionEvent.AXIS_HAT_X) }.getOrDefault(0f)
        val hatY = runCatching { event.getAxisValue(MotionEvent.AXIS_HAT_Y) }.getOrDefault(0f)
        val z = runCatching { event.getAxisValue(MotionEvent.AXIS_Z) }.getOrDefault(0f)
        val rz = runCatching { event.getAxisValue(MotionEvent.AXIS_RZ) }.getOrDefault(0f)
        val rx = runCatching { event.getAxisValue(MotionEvent.AXIS_RX) }.getOrDefault(0f)
        val ry = runCatching { event.getAxisValue(MotionEvent.AXIS_RY) }.getOrDefault(0f)
        mutateState { current ->
            current.copy(
                metrics = current.metrics.copy(
                    lastGamepadMotion =
                        "activity motion action=${event.actionMasked} src=0x${sources.toString(16)} " +
                            "hat(${String.format("%.2f", hatX)},${String.format("%.2f", hatY)}) " +
                            "xy(${String.format("%.2f", x)},${String.format("%.2f", y)}) " +
                            "zr(${String.format("%.2f", z)},${String.format("%.2f", rz)}) " +
                            "rxy(${String.format("%.2f", rx)},${String.format("%.2f", ry)}) $deviceName",
                ),
            )
        }
    }

    private fun handleGamepadKeyEvent(event: KeyEvent): Boolean {
        if (event.action != KeyEvent.ACTION_DOWN && event.action != KeyEvent.ACTION_UP) {
            return false
        }
        if (event.action == KeyEvent.ACTION_DOWN && event.repeatCount > 0) {
            return true
        }

        val mask = resolveGamepadButtonMask(event)
        publishLastGamepadKey(event, mask)
        if (mask == 0) {
            return false
        }

        synchronized(sync) {
            val controllerId = resolveControllerIdLocked(event.deviceId, event.device)
            val current = getOrCreateGamepadStateLocked(controllerId)
            lastActiveControllerId = controllerId
            gamepadStates[controllerId] = if (event.action == KeyEvent.ACTION_DOWN) {
                current.copy(keyButtons = current.keyButtons or mask)
            } else {
                current.copy(keyButtons = current.keyButtons and mask.inv())
            }
        }
        emitGamepadStateIfChanged(resolveControllerId(event.deviceId, event.device))
        return true
    }

    private fun publishLastGamepadKey(event: KeyEvent, mask: Int) {
        val actionLabel = when (event.action) {
            KeyEvent.ACTION_DOWN -> "down"
            KeyEvent.ACTION_UP -> "up"
            else -> event.action.toString()
        }
        val deviceName = event.device?.name ?: "unknown"
        val label = "pad ${event.deviceId} key ${event.keyCode} $actionLabel mask 0x${mask.toString(16)} $deviceName"
        mutateState { current ->
            current.copy(
                metrics = current.metrics.copy(
                    lastGamepadKey = label,
                ),
            )
        }
    }

    private fun resolveGamepadButtonMask(event: KeyEvent): Int {
        val isDualSense = event.device?.name?.contains("dualsense", ignoreCase = true) == true
        return synchronized(sync) {
            when (event.keyCode) {
                KeyEvent.KEYCODE_BUTTON_A -> {
                    if (event.action == KeyEvent.ACTION_DOWN) {
                        sawStandardFaceButtonA = true
                        sawStandardFaceButtons = true
                    }
                    if (isDualSense) XINPUT_GAMEPAD_X else XINPUT_GAMEPAD_A
                }

                KeyEvent.KEYCODE_BUTTON_1 -> XINPUT_GAMEPAD_A

                KeyEvent.KEYCODE_BUTTON_B -> {
                    if (event.action == KeyEvent.ACTION_DOWN) {
                        sawStandardFaceButtons = true
                        sawStandardFaceButtonB = true
                    }
                    if (isDualSense) XINPUT_GAMEPAD_A else if (shouldUseDualSenseTvFaceFallback(event)) XINPUT_GAMEPAD_A else XINPUT_GAMEPAD_B
                }

                KeyEvent.KEYCODE_BUTTON_2 -> XINPUT_GAMEPAD_B

                KeyEvent.KEYCODE_BUTTON_C -> {
                    if (sawStandardFaceButtonB) {
                        0
                    } else {
                        XINPUT_GAMEPAD_B
                    }
                }

                KeyEvent.KEYCODE_BACK -> XINPUT_GAMEPAD_B

                KeyEvent.KEYCODE_BUTTON_X -> {
                    if (event.action == KeyEvent.ACTION_DOWN) {
                        sawStandardFaceButtons = true
                    }
                    if (isDualSense) XINPUT_GAMEPAD_Y else XINPUT_GAMEPAD_X
                }

                KeyEvent.KEYCODE_BUTTON_3 -> XINPUT_GAMEPAD_X

                KeyEvent.KEYCODE_BUTTON_Y -> {
                    if (event.action == KeyEvent.ACTION_DOWN) {
                        sawStandardFaceButtons = true
                    }
                    if (isDualSense) XINPUT_GAMEPAD_X else XINPUT_GAMEPAD_Y
                }

                KeyEvent.KEYCODE_BUTTON_4 -> XINPUT_GAMEPAD_Y

                KeyEvent.KEYCODE_BUTTON_L1, KeyEvent.KEYCODE_BUTTON_5 -> XINPUT_GAMEPAD_LEFT_SHOULDER
                KeyEvent.KEYCODE_BUTTON_R1, KeyEvent.KEYCODE_BUTTON_6 -> XINPUT_GAMEPAD_RIGHT_SHOULDER
                KeyEvent.KEYCODE_BUTTON_L2, KeyEvent.KEYCODE_BUTTON_7 -> XINPUT_GAMEPAD_LEFT_SHOULDER
                KeyEvent.KEYCODE_BUTTON_R2, KeyEvent.KEYCODE_BUTTON_8 -> XINPUT_GAMEPAD_RIGHT_SHOULDER
                KeyEvent.KEYCODE_BUTTON_START, KeyEvent.KEYCODE_BUTTON_MODE, KeyEvent.KEYCODE_BUTTON_10 -> XINPUT_GAMEPAD_START
                KeyEvent.KEYCODE_BUTTON_SELECT, KeyEvent.KEYCODE_BUTTON_9 -> XINPUT_GAMEPAD_BACK
                KeyEvent.KEYCODE_BUTTON_THUMBL, KeyEvent.KEYCODE_BUTTON_11 -> XINPUT_GAMEPAD_LEFT_THUMB
                KeyEvent.KEYCODE_BUTTON_THUMBR, KeyEvent.KEYCODE_BUTTON_12 -> XINPUT_GAMEPAD_RIGHT_THUMB
                KeyEvent.KEYCODE_DPAD_UP -> XINPUT_GAMEPAD_DPAD_UP
                KeyEvent.KEYCODE_DPAD_DOWN -> XINPUT_GAMEPAD_DPAD_DOWN
                KeyEvent.KEYCODE_DPAD_LEFT -> XINPUT_GAMEPAD_DPAD_LEFT
                KeyEvent.KEYCODE_DPAD_RIGHT -> XINPUT_GAMEPAD_DPAD_RIGHT
                KeyEvent.KEYCODE_DPAD_CENTER -> if (isDualSense) XINPUT_GAMEPAD_B else 0
                KeyEvent.KEYCODE_SYSTEM_NAVIGATION_UP -> XINPUT_GAMEPAD_DPAD_UP
                KeyEvent.KEYCODE_SYSTEM_NAVIGATION_DOWN -> XINPUT_GAMEPAD_DPAD_DOWN
                KeyEvent.KEYCODE_SYSTEM_NAVIGATION_LEFT -> XINPUT_GAMEPAD_DPAD_LEFT
                KeyEvent.KEYCODE_SYSTEM_NAVIGATION_RIGHT -> XINPUT_GAMEPAD_DPAD_RIGHT
                else -> 0
            }
        }
    }

    private fun shouldTreatBackAsGamepadCircle(event: KeyEvent): Boolean {
        return !event.isFromSource(InputDevice.SOURCE_MOUSE)
    }

    private fun shouldUseDualSenseTvFaceFallback(event: KeyEvent): Boolean {
        val deviceName = event.device?.name.orEmpty()
        if (!deviceName.contains("dual", ignoreCase = true)) {
            return false
        }
        return !sawStandardFaceButtonA
    }

    private fun isGamepadLikeEvent(event: KeyEvent): Boolean {
        if (event.isFromSource(InputDevice.SOURCE_GAMEPAD) ||
            event.isFromSource(InputDevice.SOURCE_JOYSTICK) ||
            event.isFromSource(InputDevice.SOURCE_DPAD)
        ) {
            return true
        }

        val deviceSources = event.device?.sources ?: 0
        if ((deviceSources and InputDevice.SOURCE_GAMEPAD) == InputDevice.SOURCE_GAMEPAD ||
            (deviceSources and InputDevice.SOURCE_JOYSTICK) == InputDevice.SOURCE_JOYSTICK ||
            (deviceSources and InputDevice.SOURCE_DPAD) == InputDevice.SOURCE_DPAD
        ) {
            return true
        }

        val deviceName = event.device?.name.orEmpty()
        val controllerLikeDevice =
            deviceName.contains("dual", ignoreCase = true) ||
                deviceName.contains("controller", ignoreCase = true) ||
                deviceName.contains("gamepad", ignoreCase = true)

        return when (event.keyCode) {
            KeyEvent.KEYCODE_BUTTON_A,
            KeyEvent.KEYCODE_BUTTON_1,
            KeyEvent.KEYCODE_BUTTON_B,
            KeyEvent.KEYCODE_BUTTON_2,
            KeyEvent.KEYCODE_BUTTON_C,
            KeyEvent.KEYCODE_BUTTON_X,
            KeyEvent.KEYCODE_BUTTON_3,
            KeyEvent.KEYCODE_BUTTON_Y,
            KeyEvent.KEYCODE_BUTTON_4,
            KeyEvent.KEYCODE_BUTTON_L1,
            KeyEvent.KEYCODE_BUTTON_5,
            KeyEvent.KEYCODE_BUTTON_R1,
            KeyEvent.KEYCODE_BUTTON_6,
            KeyEvent.KEYCODE_BUTTON_L2,
            KeyEvent.KEYCODE_BUTTON_7,
            KeyEvent.KEYCODE_BUTTON_R2,
            KeyEvent.KEYCODE_BUTTON_8,
            KeyEvent.KEYCODE_BUTTON_9,
            KeyEvent.KEYCODE_BUTTON_10,
            KeyEvent.KEYCODE_BUTTON_11,
            KeyEvent.KEYCODE_BUTTON_12,
            KeyEvent.KEYCODE_BUTTON_THUMBL,
            KeyEvent.KEYCODE_BUTTON_THUMBR,
            KeyEvent.KEYCODE_BUTTON_START,
            KeyEvent.KEYCODE_BUTTON_SELECT,
            KeyEvent.KEYCODE_BUTTON_MODE,
            KeyEvent.KEYCODE_DPAD_UP,
            KeyEvent.KEYCODE_DPAD_DOWN,
            KeyEvent.KEYCODE_DPAD_LEFT,
            KeyEvent.KEYCODE_DPAD_RIGHT,
            KeyEvent.KEYCODE_DPAD_CENTER -> true
            KeyEvent.KEYCODE_SYSTEM_NAVIGATION_UP,
            KeyEvent.KEYCODE_SYSTEM_NAVIGATION_DOWN,
            KeyEvent.KEYCODE_SYSTEM_NAVIGATION_LEFT,
            KeyEvent.KEYCODE_SYSTEM_NAVIGATION_RIGHT -> true
            KeyEvent.KEYCODE_BACK -> event.device != null && !event.isFromSource(InputDevice.SOURCE_MOUSE)
            else -> false
        }
    }

    private fun isGamepadMotionEvent(event: MotionEvent): Boolean {
        if (event.isFromSource(InputDevice.SOURCE_GAMEPAD) ||
            event.isFromSource(InputDevice.SOURCE_JOYSTICK) ||
            event.isFromSource(InputDevice.SOURCE_DPAD)
        ) {
            return true
        }

        val deviceSources = event.device?.sources ?: 0
        return (deviceSources and InputDevice.SOURCE_GAMEPAD) == InputDevice.SOURCE_GAMEPAD ||
            (deviceSources and InputDevice.SOURCE_JOYSTICK) == InputDevice.SOURCE_JOYSTICK ||
            (deviceSources and InputDevice.SOURCE_DPAD) == InputDevice.SOURCE_DPAD
    }

    private fun updateGamepadAxes(event: MotionEvent) {
        val controllerId = synchronized(sync) { resolveControllerIdLocked(event.deviceId, event.device) }
        val rawX = readAxis(event, MotionEvent.AXIS_X)
        val rawY = readAxis(event, MotionEvent.AXIS_Y)
        val leftX = axisToThumb(rawX)
        val leftY = axisToThumb(-rawY)
        val rightX = axisToThumb(firstNonZeroAxis(event, MotionEvent.AXIS_Z, MotionEvent.AXIS_RX))
        val rightY = axisToThumb(-firstNonZeroAxis(event, MotionEvent.AXIS_RZ, MotionEvent.AXIS_RY))
        val leftTrigger = axisToTrigger(firstNonZeroAxis(event, MotionEvent.AXIS_LTRIGGER, MotionEvent.AXIS_BRAKE))
        val rightTrigger = axisToTrigger(firstNonZeroAxis(event, MotionEvent.AXIS_RTRIGGER, MotionEvent.AXIS_GAS))
        val rawHatX = readAxis(event, MotionEvent.AXIS_HAT_X)
        val rawHatY = readAxis(event, MotionEvent.AXIS_HAT_Y)
        val altHatX = selectDigitalAxis(rawHatX, readAxis(event, MotionEvent.AXIS_RX), readAxis(event, MotionEvent.AXIS_Z))
        val altHatY = selectDigitalAxis(rawHatY, readAxis(event, MotionEvent.AXIS_RY), readAxis(event, MotionEvent.AXIS_RZ))
        val xyFallbackX = digitalAxisFromStick(rawX, rawY)
        val xyFallbackY = digitalAxisFromStick(rawY, rawX)
        val hatX = when {
            kotlin.math.abs(rawHatX) >= 0.5f -> rawHatX
            kotlin.math.abs(altHatX) >= 0.5f -> altHatX
            kotlin.math.abs(xyFallbackX) >= 0.5f -> xyFallbackX
            else -> 0f
        }
        val hatY = when {
            kotlin.math.abs(rawHatY) >= 0.5f -> rawHatY
            kotlin.math.abs(altHatY) >= 0.5f -> altHatY
            kotlin.math.abs(xyFallbackY) >= 0.5f -> xyFallbackY
            else -> 0f
        }
        var motionButtons = synchronized(sync) { getOrCreateGamepadStateLocked(controllerId).motionButtons }
        motionButtons = motionButtons and XINPUT_GAMEPAD_DPAD_LEFT.inv()
        motionButtons = motionButtons and XINPUT_GAMEPAD_DPAD_RIGHT.inv()
        motionButtons = motionButtons and XINPUT_GAMEPAD_DPAD_UP.inv()
        motionButtons = motionButtons and XINPUT_GAMEPAD_DPAD_DOWN.inv()
        if (hatX <= -0.5f) motionButtons = motionButtons or XINPUT_GAMEPAD_DPAD_LEFT
        if (hatX >= 0.5f) motionButtons = motionButtons or XINPUT_GAMEPAD_DPAD_RIGHT
        if (hatY <= -0.5f) motionButtons = motionButtons or XINPUT_GAMEPAD_DPAD_UP
        if (hatY >= 0.5f) motionButtons = motionButtons or XINPUT_GAMEPAD_DPAD_DOWN

        synchronized(sync) {
            val current = getOrCreateGamepadStateLocked(controllerId)
            lastActiveControllerId = controllerId
            gamepadStates[controllerId] = current.copy(
                motionButtons = motionButtons,
                leftTrigger = leftTrigger,
                rightTrigger = rightTrigger,
                leftThumbX = leftX,
                leftThumbY = leftY,
                rightThumbX = rightX,
                rightThumbY = rightY,
            )
        }
        val hasAxisActivity =
            kotlin.math.abs(leftX) >= 4096 ||
                kotlin.math.abs(leftY) >= 4096 ||
                kotlin.math.abs(rightX) >= 4096 ||
                kotlin.math.abs(rightY) >= 4096 ||
                leftTrigger >= 8 ||
                rightTrigger >= 8
        if (motionButtons != 0 ||
            kotlin.math.abs(hatX) >= 0.5f ||
            kotlin.math.abs(hatY) >= 0.5f ||
            kotlin.math.abs(rawHatX) >= 0.5f ||
            kotlin.math.abs(rawHatY) >= 0.5f ||
            hasAxisActivity
        ) {
            mutateState { current ->
                current.copy(
                    metrics = current.metrics.copy(
                        lastGamepadMotion = buildString {
                            append("pad ")
                            append(controllerId)
                            append(" motion hat(")
                            append(String.format("%.2f", hatX))
                            append(',')
                            append(String.format("%.2f", hatY))
                            append(") raw(")
                            append(String.format("%.2f", rawHatX))
                            append(',')
                            append(String.format("%.2f", rawHatY))
                            append(") xy(")
                            append(String.format("%.2f", rawX))
                            append(',')
                            append(String.format("%.2f", rawY))
                            append(") thumb(")
                            append(leftX)
                            append(',')
                            append(leftY)
                            append(") rthumb(")
                            append(rightX)
                            append(',')
                            append(rightY)
                            append(") trig(")
                            append(leftTrigger)
                            append(',')
                            append(rightTrigger)
                            append(") buttons=0x")
                            append((synchronized(sync) { getOrCreateGamepadStateLocked(controllerId).buttons }).toString(16))
                        },
                    ),
                )
            }
        }
        emitGamepadStateIfChanged(controllerId)
    }

    private fun emitGamepadStateIfChanged(controllerId: Int) {
        val snapshot = synchronized(sync) {
            val current = getOrCreateGamepadStateLocked(controllerId)
            val previous = lastSentGamepadStates[controllerId]
            if (current == previous) {
                return
            }
            lastSentGamepadStates[controllerId] = current
            current
        }

        enqueueControl(
            buildRemoteGamepadState(
                nextInputSeq(),
                snapshot.controllerId,
                snapshot.buttons.toUShort(),
                snapshot.leftTrigger.coerceIn(0, 255),
                snapshot.rightTrigger.coerceIn(0, 255),
                snapshot.leftThumbX.coerceIn(Short.MIN_VALUE.toInt(), Short.MAX_VALUE.toInt()),
                snapshot.leftThumbY.coerceIn(Short.MIN_VALUE.toInt(), Short.MAX_VALUE.toInt()),
                snapshot.rightThumbX.coerceIn(Short.MIN_VALUE.toInt(), Short.MAX_VALUE.toInt()),
                snapshot.rightThumbY.coerceIn(Short.MIN_VALUE.toInt(), Short.MAX_VALUE.toInt()),
            ),
        )
    }

    private fun resolveControllerId(deviceId: Int, device: InputDevice?): Int =
        synchronized(sync) { resolveControllerIdLocked(deviceId, device) }

    private fun resolveControllerIdLocked(deviceId: Int, device: InputDevice?): Int {
        controllerSlotByDeviceId[deviceId]?.let { return it }
        val bindingKey = buildControllerBindingKey(deviceId, device)
        controllerSlotByBindingKey[bindingKey]?.let { slot ->
            controllerSlotByDeviceId[deviceId] = slot
            return slot
        }
        val usedSlots = controllerSlotByBindingKey.values.toSet()
        val slot = sequenceOf(0, 1, 2, 3).firstOrNull { it !in usedSlots } ?: deviceId.coerceAtLeast(0)
        controllerSlotByBindingKey[bindingKey] = slot
        controllerSlotByDeviceId[deviceId] = slot
        return slot
    }

    private fun buildControllerBindingKey(deviceId: Int, device: InputDevice?): ControllerBindingKey {
        val resolvedDevice = device ?: InputDevice.getDevice(deviceId)
        return ControllerBindingKey(
            descriptor = resolvedDevice?.descriptor?.takeIf { it.isNotBlank() } ?: "device:$deviceId",
            vendorId = resolvedDevice?.vendorId ?: 0,
            productId = resolvedDevice?.productId ?: 0,
            name = resolvedDevice?.name ?: "unknown",
        )
    }

    private fun getOrCreateGamepadStateLocked(controllerId: Int): RemoteGamepadState {
        return gamepadStates.getOrPut(controllerId) { RemoteGamepadState(controllerId = controllerId) }
    }

    private fun readAxis(event: MotionEvent, axis: Int): Float = runCatching { event.getAxisValue(axis) }.getOrDefault(0f)

    private fun firstNonZeroAxis(event: MotionEvent, primary: Int, secondary: Int): Float {
        val primaryValue = readAxis(event, primary)
        return if (kotlin.math.abs(primaryValue) >= 0.01f) primaryValue else readAxis(event, secondary)
    }

    private fun axisToThumb(value: Float): Int {
        val deadzoned = if (kotlin.math.abs(value) < 0.08f) 0f else value
        return (deadzoned.coerceIn(-1f, 1f) * Short.MAX_VALUE).toInt()
    }

    private fun axisToTrigger(value: Float): Int {
        val normalized = if (value <= 0f) 0f else value.coerceIn(0f, 1f)
        return (normalized * 255f).toInt()
    }

    private fun selectDigitalAxis(primary: Float, firstFallback: Float, secondFallback: Float): Float {
        if (kotlin.math.abs(primary) >= 0.5f) {
            return primary
        }
        if (kotlin.math.abs(firstFallback) >= 0.5f) {
            return firstFallback
        }
        if (kotlin.math.abs(secondFallback) >= 0.5f) {
            return secondFallback
        }
        return primary
    }

    private fun digitalAxisFromStick(primary: Float, orthogonal: Float): Float {
        return if (kotlin.math.abs(primary) >= 0.85f && kotlin.math.abs(orthogonal) <= 0.45f) {
            primary
        } else {
            0f
        }
    }

    private fun receiveLoop(datagramSocket: DatagramSocket) {
        val buffer = ByteArray(TransportProtocol.MAX_PACKET_SIZE)
        while (true) {
            val isRunning = synchronized(sync) { running }
            if (!isRunning) {
                return
            }

            val packet = DatagramPacket(buffer, buffer.size)
            try {
                datagramSocket.receive(packet)
            } catch (_: java.net.SocketTimeoutException) {
                continue
            } catch (_: IOException) {
                return
            } catch (error: Exception) {
                markError("Receiver failed: ${error.message ?: error.javaClass.simpleName}")
                return
            }

            var shouldLogFirstRawPacket = false
            synchronized(sync) {
                if (!loggedFirstRawUdpPacket) {
                    loggedFirstRawUdpPacket = true
                    shouldLogFirstRawPacket = true
                }
            }
            if (shouldLogFirstRawPacket) {
                eventListener?.invoke(
                    "FIRST UDP PACKET | from=${packet.address.hostAddress ?: "-"}:${packet.port} | bytes=${packet.length}",
                )
            }

            val evrtPacket = EvrtPacket.parse(packet.data, packet.length) ?: continue
            synchronized(sync) {
                remoteEndpoint = InetSocketAddress(packet.address, packet.port)
                lastPacketReceivedAtNs = System.nanoTime()
            }
            updateConnectedEndpoint(packet.address.hostAddress ?: "-", packet.port)
            logFirstTransportPacket(evrtPacket.type, packet)
            if (evrtPacket.type == TransportProtocol.TYPE_SESSION_CONFIG ||
                evrtPacket.type == TransportProtocol.TYPE_CODEC_CONFIG ||
                evrtPacket.type == TransportProtocol.TYPE_VIDEO_FRAME ||
                evrtPacket.type == TransportProtocol.TYPE_AUDIO_CONFIG ||
                evrtPacket.type == TransportProtocol.TYPE_AUDIO_FRAME
            ) {
                markTransportPacketSeen(packet.address.hostAddress ?: "-", packet.port)
            }

            when (evrtPacket.type) {
                TransportProtocol.TYPE_SESSION_CONFIG -> handleSessionConfig(evrtPacket.payload)
                TransportProtocol.TYPE_CODEC_CONFIG -> handleCodecConfig(evrtPacket.payload)
                TransportProtocol.TYPE_VIDEO_FRAME -> handleVideoPacket(evrtPacket)
                TransportProtocol.TYPE_CONTROL -> handleControlPacket(evrtPacket.payload, packet.address, packet.port)
                TransportProtocol.TYPE_AUDIO_CONFIG -> handleAudioConfig(evrtPacket.payload)
                TransportProtocol.TYPE_AUDIO_FRAME -> handleAudioPacket(evrtPacket)
            }
        }
    }

    private fun handleSessionConfig(payload: ByteArray) {
        val config = SessionConfig.parse(payload) ?: return
        eventListener?.invoke(
            "SESSION CONFIG | codec=${config.codecLabel} | size=${config.width}x${config.height} | fps=${config.fps}",
        )
        val shouldApplyConfig = synchronized(sync) {
            shouldApplySessionConfigLocked(config)
        }
        if (!shouldApplyConfig) {
            return
        }

        synchronized(sync) {
            applySessionConfigLocked(config)
        }

        mutateState { current ->
            current.copy(
                phase = PcReceiverPhase.CONNECTED,
                status = "",
                metrics = current.metrics.copy(
                    resolutionLabel = "${config.width}x${config.height}",
                    codecLabel = config.codecLabel,
                    videoWidth = config.width,
                    videoHeight = config.height,
                    pulseToAndroidEstimateMs = -1,
                    inputToAndroidEstimateMs = -1,
                ),
                lastError = null,
            )
        }
        mutateDiagnostics { current ->
            current.copy(firstPacketReceived = true)
        }

        maybeConfigureDecoder(force = true)
        sendRequestKeyFrame()
        mainHandler.postDelayed(
            {
                if (uiState.phase == PcReceiverPhase.CONNECTED) {
                    requestLatencyMeasurementInternal("android_auto", force = false)
                }
            },
            700L,
        )
    }

    private fun handleCodecConfig(payload: ByteArray) {
        eventListener?.invoke("CODEC CONFIG | bytes=${payload.size}")
        val shouldReconfigure = synchronized(sync) {
            val existing = codecConfig
            if (existing != null && existing.contentEquals(payload)) {
                false
            } else {
                codecConfig = payload.copyOf()
                waitingForKeyFrame = true
                lastLatencyRequestSentAtNs = 0L
                pulseToAndroidEstimateMs = -1
                inputToAndroidEstimateMs = -1
                recentPulseToAndroidEstimates.clear()
                recentInputToAndroidEstimates.clear()
                frameArrivalNsByPts.clear()
                framePresentedNsByPts.clear()
                pendingLatencyPulses.clear()
                latencyRequestStartedAtNsBySeq.clear()
                true
            }
        }

        if (!shouldReconfigure) {
            return
        }

        mutateState { current ->
            current.copy(
                metrics = current.metrics.copy(
                    pulseToAndroidEstimateMs = -1,
                    inputToAndroidEstimateMs = -1,
                ),
            )
        }

        maybeConfigureDecoder(force = true)
        sendRequestKeyFrame()
    }

    private fun shouldApplySessionConfigLocked(config: SessionConfig): Boolean {
        val current = sessionConfig
        if (current == null) {
            return true
        }

        if (current.codec != config.codec) {
            eventListener?.invoke("SESSION CONFIG APPLY | codec changed ${current.codecLabel} -> ${config.codecLabel}")
            return true
        }

        if (current.width == config.width && current.height == config.height && current.fps == config.fps && current.preset == config.preset) {
            pendingSessionConfig = null
            return false
        }

        val nowNs = System.nanoTime()
        val widthDelta = kotlin.math.abs(current.width - config.width)
        val heightDelta = kotlin.math.abs(current.height - config.height)
        val widthDeltaRatio = widthDelta.toDouble() / max(1, current.width)
        val heightDeltaRatio = heightDelta.toDouble() / max(1, current.height)
        val minorResize = widthDeltaRatio < 0.12 && heightDeltaRatio < 0.12
        val recentApply = nowNs - lastSessionConfigAppliedAtNs < 1_500_000_000L

        if (minorResize && recentApply) {
            val pending = pendingSessionConfig
            return if (pending == null || pending != config) {
                pendingSessionConfig = config
                eventListener?.invoke("SESSION CONFIG HOLD | current=${current.width}x${current.height}@${current.fps} pending=${config.width}x${config.height}@${config.fps}")
                false
            } else {
                eventListener?.invoke("SESSION CONFIG APPLY | stable pending=${config.width}x${config.height}@${config.fps}")
                pendingSessionConfig = null
                true
            }
        }

        pendingSessionConfig = null
        return true
    }

    private fun applySessionConfigLocked(config: SessionConfig) {
        sessionConfig = config
        lastSessionConfigAppliedAtNs = System.nanoTime()
        waitingForKeyFrame = true
        lastLatencyRequestSentAtNs = 0L
        pulseToAndroidEstimateMs = -1
        inputToAndroidEstimateMs = -1
        frameAssembler.reset()
        audioFrameAssembler.reset()
        recentPulseToAndroidEstimates.clear()
        recentInputToAndroidEstimates.clear()
        frameArrivalNsByPts.clear()
        framePresentedNsByPts.clear()
        pendingLatencyPulses.clear()
        latencyRequestStartedAtNsBySeq.clear()
    }

    private fun handleVideoPacket(packet: EvrtPacket) {
        frameAssembler.push(packet)
        mutateState { current ->
            current.copy(
                metrics = current.metrics.copy(
                    packetsReceived = current.metrics.packetsReceived + 1,
                ),
            )
        }
    }

    private fun handleControlPacket(payload: ByteArray, sourceAddress: java.net.InetAddress, sourcePort: Int) {
        val text = runCatching { payload.toString(Charsets.UTF_8) }.getOrNull() ?: return
        val json = runCatching { org.json.JSONObject(text) }.getOrNull() ?: return
        when (json.optString("kind", "")) {
            "latency_pulse" -> handleLatencyPulse(json)
            "discovery_probe" -> sendDiscoveryResponse(sourceAddress, sourcePort)
            "relay_register_ack" -> {
                val ackSessionId = json.optString("sessionId").trim()
                val observedAddress = json.optString("observedAddress").trim()
                val observedPort = json.optInt("observedPort", 0)
                val observedEndpoint = if (observedAddress.isNotBlank() && observedPort > 0) {
                    "$observedAddress:$observedPort"
                } else {
                    "${sourceAddress.hostAddress ?: "-"}:$sourcePort"
                }
                mutateDiagnostics { current ->
                    current.copy(
                        sessionId = ackSessionId.ifBlank { current.sessionId },
                        receiverRegistered = true,
                        lastRelayAck = observedEndpoint,
                    )
                }
                eventListener?.invoke("RELAY REGISTER ACK | session=${ackSessionId.ifBlank { "-" }} | observed=$observedEndpoint")
            }
        }
    }

    private fun sendDiscoveryResponse(address: java.net.InetAddress, port: Int) {
        val payload =
            """{"kind":"discovery_response","deviceName":"Android PC Receiver","role":"android_receiver","port":${uiState.listenPort}}"""
                .toByteArray(Charsets.UTF_8)
        val packet = ByteArray(TransportProtocol.HEADER_SIZE + payload.size)
        val header = ByteBuffer.wrap(packet).order(ByteOrder.BIG_ENDIAN)
        header.putInt(TransportProtocol.MAGIC)
        header.put(TransportProtocol.VERSION)
        header.put(TransportProtocol.TYPE_CONTROL)
        header.putShort(0)
        header.putInt(0)
        header.putShort(0)
        header.putShort(1)
        header.putLong(0L)
        System.arraycopy(payload, 0, packet, TransportProtocol.HEADER_SIZE, payload.size)
        synchronized(sync) {
            socket?.send(DatagramPacket(packet, packet.size, address, port))
        }
    }

    private fun handleLatencyPulse(json: org.json.JSONObject) {
        val presentationTimeUs = json.optLong("presentationTimeUs", 0L)
        if (presentationTimeUs <= 0L) {
            return
        }

        val nowNs = System.nanoTime()
        synchronized(sync) {
            pendingLatencyPulses[presentationTimeUs] = PendingLatencyPulse(
                pulseId = json.optLong("pulseId", 0L),
                source = json.optString("source", "manual"),
                presentationTimeUs = presentationTimeUs,
                senderPipelineMs = json.optInt("senderPipelineMs", 0).coerceAtLeast(0),
                approxSenderMs = json.optInt("approxSenderMs", 0).coerceAtLeast(0),
                inputSeq = json.optLong("inputSeq", 0L).coerceAtLeast(0L),
                receivedAtNs = nowNs,
            )
            trimLatencyMapsLocked(nowNs)
        }
        maybeFinalizeLatencyEstimate(presentationTimeUs, nowNs)
    }

    private fun handleAudioConfig(payload: ByteArray) {
        val config = PcReceiverAudioConfig.parse(payload) ?: return
        audioFrameAssembler.reset()
        try {
            val lowLatencyMode = synchronized(sync) { sessionConfig?.isGame } ?: false
            audioPlaybackSink.applyConfig(config, lowLatencyMode)
            mutateState { current ->
                current.copy(
                    metrics = current.metrics.copy(
                        audioStatus = "${config.sampleRate} Hz / ${if (config.channels > 1) 2 else 1} ch",
                    ),
                )
            }
        } catch (error: Exception) {
            mutateState { current ->
                current.copy(
                    metrics = current.metrics.copy(
                        audioStatus = "Unavailable: ${error.message ?: error.javaClass.simpleName}",
                    ),
                )
            }
        }
    }

    private fun handleAudioPacket(packet: EvrtPacket) {
        audioFrameAssembler.onPacket(packet)
        mutateState { current ->
            current.copy(
                metrics = current.metrics.copy(
                    audioPacketsReceived = current.metrics.audioPacketsReceived + 1,
                ),
            )
        }
    }

    private fun onAccessUnitReady(bytes: ByteArray, presentationTimeUs: Long, isKeyFrame: Boolean) {
        val nowNs = System.nanoTime()
        val codecPrefix = synchronized(sync) {
            when {
                isKeyFrame -> {
                    waitingForKeyFrame = false
                    codecConfig?.copyOf()
                }
                waitingForKeyFrame -> return
                else -> null
            }
        }

        val accessUnit = if (codecPrefix != null && codecPrefix.isNotEmpty()) {
            ByteArray(codecPrefix.size + bytes.size).also { combined ->
                System.arraycopy(codecPrefix, 0, combined, 0, codecPrefix.size)
                System.arraycopy(bytes, 0, combined, codecPrefix.size, bytes.size)
            }
        } else {
            bytes
        }

        synchronized(sync) {
            if (!isKeyFrame && presentationTimeUs <= lastQueuedPresentationTimeUs) {
                return
            }
            frameArrivalNsByPts[presentationTimeUs] = nowNs
            trimLatencyMapsLocked(nowNs)
        }

        val nativeDecoder = synchronized(sync) { nativeDecoder }
        if (nativeDecoder != null) {
            try {
                val result = nativeDecoder.decodeAccessUnit(accessUnit, presentationTimeUs)
                if (result.needsDrop) {
                    onFrameDropped()
                    return
                }

                if (result.outputWidth > 0 && result.outputHeight > 0) {
                    handleDecoderOutputFormatChanged(result.outputWidth, result.outputHeight)
                }
                if (result.renderedFrames > 0) {
                    handleDecoderRenderedFrame(
                        presentationTimeUs = if (result.lastRenderedPtsUs >= 0) result.lastRenderedPtsUs else presentationTimeUs,
                        renderedFrames = result.renderedFrames,
                    )
                }
                return
            } catch (error: Exception) {
                markError("Native decoder input failed: ${error.message ?: error.javaClass.simpleName}")
                return
            }
        }

        val codec = synchronized(sync) { decoder } ?: return
        try {
            drainDecoder(codec)
            var inputIndex = codec.dequeueInputBuffer(0)
            if (inputIndex < 0) {
                drainDecoder(codec)
                inputIndex = codec.dequeueInputBuffer(0)
            }
            if (inputIndex < 0) {
                onFrameDropped()
                return
            }

            val inputBuffer = codec.getInputBuffer(inputIndex) ?: run {
                onFrameDropped()
                return
            }
            if (inputBuffer.capacity() < accessUnit.size) {
                onFrameDropped()
                return
            }

            inputBuffer.clear()
            inputBuffer.put(accessUnit)
            codec.queueInputBuffer(inputIndex, 0, accessUnit.size, presentationTimeUs, 0)
            synchronized(sync) {
                if (presentationTimeUs > lastQueuedPresentationTimeUs) {
                    lastQueuedPresentationTimeUs = presentationTimeUs
                }
            }
            drainDecoder(codec)
        } catch (error: Exception) {
            markError("Decoder input failed: ${error.message ?: error.javaClass.simpleName}")
        }
    }

    private fun drainDecoder(codec: MediaCodec) {
        val bufferInfo = MediaCodec.BufferInfo()
        while (true) {
            when (val outputIndex = codec.dequeueOutputBuffer(bufferInfo, 0)) {
                MediaCodec.INFO_TRY_AGAIN_LATER -> return
                MediaCodec.INFO_OUTPUT_FORMAT_CHANGED -> {
                    val format = codec.outputFormat
                    handleDecoderOutputFormatChanged(
                        format.getInteger(MediaFormat.KEY_WIDTH),
                        format.getInteger(MediaFormat.KEY_HEIGHT),
                    )
                }

                else -> {
                    if (outputIndex >= 0) {
                        val presentationTimeUs = bufferInfo.presentationTimeUs
                        codec.releaseOutputBuffer(outputIndex, true)
                        handleDecoderRenderedFrame(presentationTimeUs, renderedFrames = 1)
                    }
                }
            }
        }
    }

    private fun maybeConfigureDecoder(force: Boolean = false) {
        val (surface, sessionConfig, codecConfig) = synchronized(sync) {
            Triple(surface, sessionConfig, codecConfig)
        }
        if (surface == null || sessionConfig == null || codecConfig == null) {
            return
        }

        val decoderConfig = DecoderConfig(sessionConfig.width, sessionConfig.height, sessionConfig.codec, codecConfig.contentHashCode())
        synchronized(sync) {
            if (decoderConfigureInFlight) {
                pendingDecoderReconfigure = true
                pendingDecoderReconfigureForce = pendingDecoderReconfigureForce || force
                eventListener?.invoke(
                    "DECODER RECONFIGURE QUEUED | mime=${sessionConfig.codec} | size=${sessionConfig.width}x${sessionConfig.height} | force=$force",
                )
                return
            }

            val blockedConfig = blockedDecoderConfig
            if (blockedConfig == decoderConfig) {
                return
            }
            val currentConfig = this.decoderConfig
            if (!force && currentConfig == decoderConfig) {
                return
            }

            decoderConfigureInFlight = true
        }

        var rerun = false
        var rerunForce = false
        try {
            releaseDecoder()
            val codecSpecificData = buildCodecSpecificData(sessionConfig.codec, codecConfig)
            val nativeDecoder = if (synchronized(sync) { preferNativeDecoder }) {
                runCatching {
                    createBestNativeDecoder(
                        codecMime = sessionConfig.codec,
                        width = sessionConfig.width,
                        height = sessionConfig.height,
                        surface = surface,
                        codecSpecificData = codecSpecificData,
                    )
                }.getOrElse { nativeError ->
                    eventListener?.invoke("NDK DECODER FALLBACK | ${nativeError.message ?: nativeError.javaClass.simpleName}")
                    null
                }
            } else {
                eventListener?.invoke("NDK DECODER SKIPPED | disabled by user setting")
                null
            }
            synchronized(sync) {
                this.nativeDecoder = nativeDecoder
                decoder = if (nativeDecoder == null) {
                    createBestDecoder(
                        codecMime = sessionConfig.codec,
                        width = sessionConfig.width,
                        height = sessionConfig.height,
                        surface = surface,
                        codecSpecificData = codecSpecificData,
                    )
                } else {
                    null
                }
                this.decoderConfig = decoderConfig
                blockedDecoderConfig = null
                blockedDecoderMessage = null
                decoderConfigureInFlight = false
                rerun = pendingDecoderReconfigure
                rerunForce = pendingDecoderReconfigureForce
                pendingDecoderReconfigure = false
                pendingDecoderReconfigureForce = false
            }
            val decoderPathLabel = synchronized(sync) {
                nativeDecoder?.decoderPath?.let { "NDK / $it" }
                    ?: decoder?.let { "Java / ${describeDecoder(it)}" }
                    ?: "-"
            }
            mutateState { current ->
                current.copy(
                    metrics = current.metrics.copy(decoderPath = decoderPathLabel),
                )
            }
            eventListener?.invoke(
                "DECODER READY | mime=${sessionConfig.codec} | size=${sessionConfig.width}x${sessionConfig.height} | path=$decoderPathLabel",
            )
        } catch (error: Exception) {
            val message = buildDecoderInitFailureMessage(sessionConfig.codecLabel, error.message ?: error.javaClass.simpleName)
            synchronized(sync) {
                blockedDecoderConfig = decoderConfig
                blockedDecoderMessage = message
                decoderConfigureInFlight = false
                rerun = pendingDecoderReconfigure
                rerunForce = pendingDecoderReconfigureForce
                pendingDecoderReconfigure = false
                pendingDecoderReconfigureForce = false
            }
            eventListener?.invoke("DECODER ERROR | $message")
            markError(message)
        }

        if (rerun) {
            mainHandler.post {
                maybeConfigureDecoder(force = rerunForce)
            }
        }
    }

    private fun createBestDecoder(
        codecMime: String,
        width: Int,
        height: Int,
        surface: Surface,
        codecSpecificData: List<ByteArray>,
    ): MediaCodec {
        val attempts = mutableListOf<String>()
        val lowLatencyEligible = codecMime.contains("avc", ignoreCase = true)
        eventListener?.invoke(
            "DECODER PROBE | mime=$codecMime | size=${width}x$height | csdCount=${codecSpecificData.size} | lowLatencyEligible=$lowLatencyEligible",
        )

        fun tryDecoder(codecName: String?, includeCodecSpecificData: Boolean): MediaCodec? {
            val label = buildString {
                append(codecName ?: "default")
                append(if (includeCodecSpecificData) " + csd" else " without csd")
            }

            return try {
                createConfiguredDecoder(
                    codecMime = codecMime,
                    width = width,
                    height = height,
                    surface = surface,
                    codecSpecificData = codecSpecificData,
                    codecName = codecName,
                    includeCodecSpecificData = includeCodecSpecificData,
                )
            } catch (error: Exception) {
                val detail = error.message ?: error.javaClass.simpleName
                attempts += "$label: $detail"
                eventListener?.invoke("DECODER TRY FAIL | mime=$codecMime | attempt=$label | detail=$detail")
                null
            }
        }

        tryDecoder(codecName = null, includeCodecSpecificData = true)?.let { return it }
        tryDecoder(codecName = null, includeCodecSpecificData = false)?.let { return it }

        val codecInfos = MediaCodecList(MediaCodecList.REGULAR_CODECS).codecInfos
            .asSequence()
            .filter { !it.isEncoder }
            .filter { info -> info.supportedTypes.any { it.equals(codecMime, ignoreCase = true) } }
            .map { it.name }
            .distinct()
            .toList()

        for (codecName in codecInfos) {
            tryDecoder(codecName = codecName, includeCodecSpecificData = true)?.let { return it }
            tryDecoder(codecName = codecName, includeCodecSpecificData = false)?.let { return it }
        }

        throw IllegalArgumentException(
            buildString {
                append("No working decoder for ")
                append(codecMime)
                if (attempts.isNotEmpty()) {
                    append(": ")
                    append(attempts.joinToString(" | "))
                }
            },
        )
    }

    private fun createBestNativeDecoder(
        codecMime: String,
        width: Int,
        height: Int,
        surface: Surface,
        codecSpecificData: List<ByteArray>,
    ): NativeVideoDecoderBridge {
        val attempts = mutableListOf<String>()
        eventListener?.invoke(
            "NDK DECODER PROBE | mime=$codecMime | size=${width}x$height | csdCount=${codecSpecificData.size}",
        )

        fun tryDecoder(codecName: String?): NativeVideoDecoderBridge? {
            val label = codecName ?: "default"
            return try {
                val decoder = NativeVideoDecoderBridge.create(
                    codecMime = codecMime,
                    width = width,
                    height = height,
                    surface = surface,
                    codecSpecificData = codecSpecificData,
                    codecName = codecName,
                )
                eventListener?.invoke("NDK DECODER TRY | codec=$label | path=${decoder.decoderPath}")
                decoder
            } catch (error: Exception) {
                val detail = error.message ?: error.javaClass.simpleName
                attempts += "$label: $detail"
                eventListener?.invoke("NDK DECODER TRY FAIL | codec=$label | detail=$detail")
                null
            }
        }

        tryDecoder(codecName = null)?.let { return it }

        val codecInfos = MediaCodecList(MediaCodecList.REGULAR_CODECS).codecInfos
            .asSequence()
            .filter { !it.isEncoder }
            .filter { info -> info.supportedTypes.any { it.equals(codecMime, ignoreCase = true) } }
            .map { it.name }
            .distinct()
            .toList()

        for (codecName in codecInfos) {
            tryDecoder(codecName)?.let { return it }
        }

        throw IllegalArgumentException(
            buildString {
                append("No working native decoder for ")
                append(codecMime)
                if (attempts.isNotEmpty()) {
                    append(": ")
                    append(attempts.joinToString(" | "))
                }
            },
        )
    }

    private fun createConfiguredDecoder(
        codecMime: String,
        width: Int,
        height: Int,
        surface: Surface,
        codecSpecificData: List<ByteArray>,
        codecName: String?,
        includeCodecSpecificData: Boolean,
    ): MediaCodec {
        val format = MediaFormat.createVideoFormat(codecMime, width, height)
        format.setInteger(MediaFormat.KEY_MAX_INPUT_SIZE, max(1, width * height * 3 / 2))
        if (includeCodecSpecificData) {
            codecSpecificData.forEachIndexed { index, bytes ->
                format.setByteBuffer("csd-$index", ByteBuffer.wrap(bytes))
            }
        }
        val aggressiveLowLatency = codecMime.contains("avc", ignoreCase = true)
        val isTvDevice = isAndroidTvDevice()
        var priorityApplied = false
        var operatingRateApplied: Int? = null
        var lowLatencyApplied = false
        if (android.os.Build.VERSION.SDK_INT >= 23) {
            if (aggressiveLowLatency) {
                runCatching {
                    format.setInteger(MediaFormat.KEY_PRIORITY, 0)
                    priorityApplied = true
                }
                val configuredFps = synchronized(sync) { sessionConfig?.fps } ?: 60
                val targetOperatingRate = max(60, configuredFps)
                runCatching {
                    format.setInteger(MediaFormat.KEY_OPERATING_RATE, targetOperatingRate)
                    operatingRateApplied = targetOperatingRate
                }
            }
        }
        if (aggressiveLowLatency && android.os.Build.VERSION.SDK_INT >= 30) {
            runCatching {
                format.setInteger(MediaFormat.KEY_LOW_LATENCY, 1)
                lowLatencyApplied = true
            }
        }
        val decoderTuningLabel =
            "profile=${if (isTvDevice) "tv" else "handheld"} " +
                "prio=${if (priorityApplied) "rt" else "off"} " +
                "opRate=${operatingRateApplied?.toString() ?: "-"} " +
                "lowLatency=${if (lowLatencyApplied) "on" else "off"}" +
                if (isTvDevice) " render=surface rollback=tv_tuning_off" else ""
        mutateState { current ->
            current.copy(metrics = current.metrics.copy(decoderTuning = decoderTuningLabel))
        }

        val codec = if (codecName != null) {
            MediaCodec.createByCodecName(codecName)
        } else {
            MediaCodec.createDecoderByType(codecMime)
        }

        try {
            val decoderPathLabel = describeDecoder(codec)
            eventListener?.invoke(
                "DECODER TRY | codec=${codec.name} | mime=$codecMime | size=${width}x$height | includeCsd=$includeCodecSpecificData | lowLatency=$aggressiveLowLatency | path=$decoderPathLabel | tuning=$decoderTuningLabel",
            )
            codec.configure(format, surface, null, 0)
            codec.start()
            return codec
        } catch (error: Exception) {
            runCatching { codec.release() }
            throw error
        }
    }

    private fun releaseDecoder() {
        val (codec, nativeDecoder) = synchronized(sync) {
            val current = decoder
            val currentNative = nativeDecoder
            decoder = null
            nativeDecoder = null
            decoderConfig = null
            lastQueuedPresentationTimeUs = Long.MIN_VALUE
            Pair(current, currentNative)
        }

        runCatching { codec?.stop() }
        runCatching { codec?.release() }
        runCatching { nativeDecoder?.close() }
    }

    private fun handleDecoderOutputFormatChanged(width: Int, height: Int) {
        mutateState { current ->
            current.copy(
                metrics = current.metrics.copy(
                    resolutionLabel = buildString {
                        append(width)
                        append('x')
                        append(height)
                    },
                    videoWidth = width,
                    videoHeight = height,
                ),
            )
        }
    }

    private fun handleDecoderRenderedFrame(presentationTimeUs: Long, renderedFrames: Int) {
        if (renderedFrames <= 0) {
            return
        }

        val now = System.nanoTime()
        synchronized(sync) {
            repeat(renderedFrames) {
                decodeTicks.addLast(now)
            }
            while (decodeTicks.size > 1 && now - decodeTicks.first() > 1_000_000_000L) {
                decodeTicks.removeFirst()
            }
            framePresentedNsByPts[presentationTimeUs] = now
            trimLatencyMapsLocked(now)
        }
        mutateState { current ->
            current.copy(
                metrics = current.metrics.copy(
                    framesDecoded = current.metrics.framesDecoded + renderedFrames,
                    decodeFps = calculateDecodeFps(now),
                ),
            )
        }
        var shouldLogDecodedFrame = false
        synchronized(sync) {
            if (!loggedFirstDecodedFrame) {
                loggedFirstDecodedFrame = true
                shouldLogDecodedFrame = true
            }
        }
        if (shouldLogDecodedFrame) {
            eventListener?.invoke(
                "FIRST DECODED FRAME | ptsUs=$presentationTimeUs | endpoint=${uiState.metrics.remoteEndpoint}",
            )
        }
        maybeFinalizeLatencyEstimate(presentationTimeUs, now)
        maybeSendReceiverFeedback(now, force = false)
    }

    private fun isAndroidTvDevice(): Boolean {
        val packageManager = context.packageManager
        if (packageManager.hasSystemFeature(PackageManager.FEATURE_TELEVISION) ||
            packageManager.hasSystemFeature(PackageManager.FEATURE_LEANBACK) ||
            packageManager.hasSystemFeature(PackageManager.FEATURE_LEANBACK_ONLY)
        ) {
            return true
        }

        val uiModeManager = context.getSystemService(UiModeManager::class.java)
        return uiModeManager?.currentModeType == Configuration.UI_MODE_TYPE_TELEVISION
    }

    private fun describeDecoder(codec: MediaCodec): String {
        val codecInfo = runCatching { codec.codecInfo }.getOrNull()
        val codecName = codecInfo?.name ?: runCatching { codec.name }.getOrDefault("unknown")
        val traits = mutableListOf<String>()

        if (codecInfo != null && android.os.Build.VERSION.SDK_INT >= 29) {
            if (codecInfo.isHardwareAccelerated) {
                traits += "hw"
            }
            if (codecInfo.isSoftwareOnly) {
                traits += "sw"
            }
            if (codecInfo.isVendor) {
                traits += "vendor"
            }
        }

        if (traits.isEmpty()) {
            val normalizedName = codecName.lowercase()
            when {
                normalizedName.startsWith("omx.google.") || normalizedName.startsWith("c2.android.") -> traits += "sw"
                normalizedName.startsWith("omx.") || normalizedName.startsWith("c2.") -> traits += "hw"
            }
        }

        return if (traits.isEmpty()) codecName else "$codecName [${traits.joinToString("/")}]"
    }

    private fun updateConnectedEndpoint(host: String, port: Int) {
        mutateState { current ->
            current.copy(
                metrics = current.metrics.copy(
                    remoteEndpoint = "$host:$port",
                ),
            )
        }
    }

    private fun markTransportPacketSeen(host: String, port: Int) {
        mutateDiagnostics { current ->
            current.copy(
                firstPacketReceived = true,
                lastMediaPacketFrom = "$host:$port",
            )
        }
        mutateState { current ->
            if (current.phase == PcReceiverPhase.REGISTERING_RELAY || current.phase == PcReceiverPhase.WAITING_FIRST_PACKET) {
                current.copy(status = "РџРѕР»СѓС‡Р°РµРј РїРѕС‚РѕРє СЃ РџРљ...")
            } else {
                current
            }
        }
    }

    private fun logFirstTransportPacket(type: Byte, packet: DatagramPacket) {
        val label = when (type) {
            TransportProtocol.TYPE_SESSION_CONFIG -> "SESSION_CONFIG"
            TransportProtocol.TYPE_CODEC_CONFIG -> "CODEC_CONFIG"
            TransportProtocol.TYPE_VIDEO_FRAME -> "VIDEO_FRAME"
            TransportProtocol.TYPE_AUDIO_CONFIG -> "AUDIO_CONFIG"
            TransportProtocol.TYPE_AUDIO_FRAME -> "AUDIO_FRAME"
            else -> return
        }

        var shouldLog = false
        synchronized(sync) {
            shouldLog = when (type) {
                TransportProtocol.TYPE_SESSION_CONFIG -> if (!loggedFirstSessionConfigPacket) {
                    loggedFirstSessionConfigPacket = true
                    true
                } else {
                    false
                }
                TransportProtocol.TYPE_CODEC_CONFIG -> if (!loggedFirstCodecConfigPacket) {
                    loggedFirstCodecConfigPacket = true
                    true
                } else {
                    false
                }
                TransportProtocol.TYPE_VIDEO_FRAME -> if (!loggedFirstVideoPacket) {
                    loggedFirstVideoPacket = true
                    true
                } else {
                    false
                }
                TransportProtocol.TYPE_AUDIO_CONFIG,
                TransportProtocol.TYPE_AUDIO_FRAME -> if (!loggedFirstAudioPacket) {
                    loggedFirstAudioPacket = true
                    true
                } else {
                    false
                }
                else -> false
            }
        }

        if (shouldLog) {
            eventListener?.invoke(
                "FIRST $label | from=${packet.address.hostAddress ?: "-"}:${packet.port} | bytes=${packet.length}",
            )
        }
    }

    private fun markError(message: String) {
        releaseDecoder()
        audioPlaybackSink.release()
        mutateState { current ->
            if (current.phase == PcReceiverPhase.ERROR && current.lastError == message) {
                current
            } else {
                current.copy(
                    phase = PcReceiverPhase.ERROR,
                    status = message,
                    lastError = message,
                )
            }
        }
    }

    private fun buildDecoderInitFailureMessage(codecLabel: String, reason: String): String {
        return if (codecLabel.contains("AV1", ignoreCase = true)) {
            "AV1 decoder unsupported on this Android device. Switch sender codec to H.265 / HEVC or H.264 / AVC. Details: $reason"
        } else if (codecLabel.contains("H.265", ignoreCase = true) || codecLabel.contains("HEVC", ignoreCase = true)) {
            "HEVC decoder unsupported on this Android device. Switch Windows sender codec to H.264 / AVC. Details: $reason"
        } else {
            "Decoder init failed: $reason"
        }
    }

    private fun onFrameDropped() {
        val now = System.nanoTime()
        synchronized(sync) {
            framesDroppedCounter += 1
        }
        mutateState { current ->
            current.copy(
                metrics = current.metrics.copy(
                    framesDropped = current.metrics.framesDropped + 1,
                ),
            )
        }
        maybeSendReceiverFeedback(now, force = true)
    }

    private fun onAudioFrameReady(bytes: ByteArray) {
        audioPlaybackSink.enqueuePcmFrame(bytes)
    }

    private fun maybeSendReceiverFeedback(nowNs: Long, force: Boolean) {
        val currentSession = synchronized(sync) { sessionConfig } ?: return
        if (uiState.phase != PcReceiverPhase.CONNECTED) {
            return
        }

        val touchControlEnabled = uiState.touchControlEnabled
        val minIntervalNs = when {
            force -> 150_000_000L
            touchControlEnabled -> 250_000_000L
            else -> 600_000_000L
        }
        val snapshot = synchronized(sync) {
            if (lastFeedbackSentAtNs != 0L && nowNs - lastFeedbackSentAtNs < minIntervalNs) {
                return
            }

            val decodeFps = calculateDecodeFps(nowNs)
            val decodeDeltaMs = if (decodeFps > 0) (1000.0 / decodeFps).toInt() else -1
            val framesDropped = framesDroppedCounter
            val newDrops = (framesDropped - lastFeedbackDropCount).coerceAtLeast(0L)
            val pressure = when {
                decodeFps in 1 until max(10, (currentSession.fps * 0.65).toInt()) -> "critical"
                decodeFps in 1 until max(12, (currentSession.fps * 0.82).toInt()) -> "high"
                newDrops >= 2L -> "critical"
                newDrops > 0L -> "high"
                else -> "normal"
            }

            lastFeedbackSentAtNs = nowNs
            lastFeedbackDropCount = framesDropped
            ReceiverFeedbackSnapshot(
                pressure = pressure,
                decodeFps = decodeFps,
                queueDrops = framesDropped,
                queueDropBurst = newDrops,
                decodeDeltaMs = decodeDeltaMs,
                presentDeltaMs = decodeDeltaMs,
                pulseEstimateMs = pulseToAndroidEstimateMs,
                inputEstimateMs = if (touchControlEnabled) inputToAndroidEstimateMs else -1,
            )
        }

        emitReceiverFeedback(snapshot)

        val shouldAutoMeasure = synchronized(sync) {
            val measureIntervalNs = when {
                !touchControlEnabled -> 2_500_000_000L
                currentSession.isGame -> 1_000_000_000L
                else -> 1_800_000_000L
            }
            lastLatencyRequestSentAtNs == 0L || nowNs - lastLatencyRequestSentAtNs >= measureIntervalNs
        }
        if (shouldAutoMeasure) {
            requestLatencyMeasurementInternal("android_auto", force = false)
        }
    }

    private fun maybeFinalizeLatencyEstimate(presentationTimeUs: Long, nowNs: Long) {
        val finalized = synchronized(sync) {
            trimLatencyMapsLocked(nowNs)
            val pulse = pendingLatencyPulses[presentationTimeUs] ?: return
            val presentedAtNs = framePresentedNsByPts[presentationTimeUs] ?: return
            val arrivedAtNs = frameArrivalNsByPts[presentationTimeUs] ?: presentedAtNs
            pendingLatencyPulses.remove(presentationTimeUs)
            framePresentedNsByPts.remove(presentationTimeUs)
            frameArrivalNsByPts.remove(presentationTimeUs)

            val receiverTailMs = ((presentedAtNs - arrivedAtNs) / 1_000_000L).toInt().coerceAtLeast(0)
            val pulseEstimate = (pulse.senderPipelineMs + receiverTailMs).coerceAtLeast(0)
            val inputEstimate = latencyRequestStartedAtNsBySeq.remove(pulse.inputSeq)?.let { startedAtNs ->
                ((presentedAtNs - startedAtNs) / 1_000_000L).toInt().coerceAtLeast(0)
            } ?: (pulse.approxSenderMs + receiverTailMs).coerceAtLeast(0)
            val decodeFps = calculateDecodeFps(presentedAtNs)
            val decodeDeltaMs = if (decodeFps > 0) (1000.0 / decodeFps).toInt() else -1

            pushLatencyEstimate(recentPulseToAndroidEstimates, pulseEstimate)
            if (uiState.touchControlEnabled) {
                pushLatencyEstimate(recentInputToAndroidEstimates, inputEstimate)
            } else {
                recentInputToAndroidEstimates.clear()
                inputToAndroidEstimateMs = -1
            }
            pulseToAndroidEstimateMs = computeMedian(recentPulseToAndroidEstimates)
            if (uiState.touchControlEnabled) {
                inputToAndroidEstimateMs = computeMedian(recentInputToAndroidEstimates)
            }
            ReceiverFeedbackSnapshot(
                pressure = "normal",
                decodeFps = decodeFps,
                queueDrops = framesDroppedCounter,
                queueDropBurst = (framesDroppedCounter - lastFeedbackDropCount).coerceAtLeast(0L),
                decodeDeltaMs = decodeDeltaMs,
                presentDeltaMs = receiverTailMs,
                pulseEstimateMs = pulseToAndroidEstimateMs,
                inputEstimateMs = if (uiState.touchControlEnabled) inputToAndroidEstimateMs else -1,
            )
        }

        mutateState { current ->
            current.copy(
                metrics = current.metrics.copy(
                    pulseToAndroidEstimateMs = finalized.pulseEstimateMs,
                    inputToAndroidEstimateMs = if (current.touchControlEnabled) finalized.inputEstimateMs else -1,
                ),
                status = if (current.touchControlEnabled) {
                    "Pulse ${finalized.pulseEstimateMs} ms / Input ${finalized.inputEstimateMs} ms"
                } else {
                    "Pulse ${finalized.pulseEstimateMs} ms / Display mode"
                },
            )
        }

        emitReceiverFeedback(finalized)
    }

    private fun emitReceiverFeedback(snapshot: ReceiverFeedbackSnapshot) {
        receiverFeedbackListener?.invoke(snapshot)
        enqueueControl(
            buildReceiverFeedback(
                pressure = snapshot.pressure,
                backlogFrames = 0,
                queueDrops = snapshot.queueDrops,
                queueDropBurst = snapshot.queueDropBurst,
                decodeFps = snapshot.decodeFps,
                assemblyDelayMs = 0,
                arrivalDeltaMs = -1,
                decodeDeltaMs = snapshot.decodeDeltaMs,
                presentDeltaMs = snapshot.presentDeltaMs,
                pulseEstimateMs = snapshot.pulseEstimateMs,
                inputEstimateMs = snapshot.inputEstimateMs,
            ),
        )
    }

    private fun calculateDecodeFps(nowNs: Long): Int {
        val ticks = synchronized(sync) { decodeTicks.toList() }
        if (ticks.size <= 1) {
            return ticks.size
        }
        val elapsedNs = nowNs - ticks.first()
        if (elapsedNs <= 0L) {
            return ticks.size
        }
        return max(1, (((ticks.size - 1) * 1_000_000_000.0) / elapsedNs).toInt())
    }

    private fun nextInputSeq(): Long = synchronized(sync) {
        inputSeq += 1
        inputSeq
    }

    private fun requestLatencyMeasurementInternal(source: String, force: Boolean) {
        val nowNs = System.nanoTime()
        val shouldSend = synchronized(sync) {
            if (!force &&
                lastLatencyRequestSentAtNs != 0L &&
                nowNs - lastLatencyRequestSentAtNs < if (uiState.touchControlEnabled) 1_000_000_000L else 3_000_000_000L
            ) {
                false
            } else {
                true
            }
        }
        if (!shouldSend || uiState.phase != PcReceiverPhase.CONNECTED) {
            return
        }

        val seq = nextInputSeq()
        synchronized(sync) {
            lastLatencyRequestSentAtNs = nowNs
            latencyRequestStartedAtNsBySeq[seq] = nowNs
            trimLatencyMapsLocked(nowNs)
        }
        enqueueControl(buildLatencyPulseRequest(seq, source))
        mutateState { current ->
            current.copy(status = "Р—Р°РїСЂРѕСЃ РёР·РјРµСЂРµРЅРёСЏ Р·Р°РґРµСЂР¶РєРё РѕС‚РїСЂР°РІР»РµРЅ")
        }
    }

    private fun sendReleaseAll() {
        enqueueControl(buildRemoteReleaseAll(nextInputSeq()))
    }

    private fun sendRequestKeyFrame() {
        enqueueControl(buildRequestKeyFrame())
    }

    private fun syncMouseButtons(buttonState: Int) {
        syncMouseButton(buttonState, MotionEvent.BUTTON_PRIMARY, "left")
        syncMouseButton(buttonState, MotionEvent.BUTTON_SECONDARY, "right")
        syncMouseButton(buttonState, MotionEvent.BUTTON_TERTIARY, "middle")
        syncMouseButton(buttonState, MotionEvent.BUTTON_BACK, "x1")
        syncMouseButton(buttonState, MotionEvent.BUTTON_FORWARD, "x2")
        lastMouseButtonState = buttonState
    }

    private fun syncMouseButton(currentState: Int, mask: Int, button: String) {
        val wasPressed = (lastMouseButtonState and mask) != 0
        val isPressed = (currentState and mask) != 0
        if (wasPressed == isPressed) {
            return
        }

        enqueueControl(buildRemoteMouseButton(nextInputSeq(), button, pressed = isPressed))
    }

    private fun enqueueControl(payload: ByteArray) {
        val handler = synchronized(sync) { controlSendHandler }
        if (handler == null) {
            sendControlDirect(payload)
            return
        }
        handler.post { sendControlDirect(payload) }
    }

    private fun enqueueAbsoluteMouseMove(payload: ByteArray) {
        val handler = synchronized(sync) { controlSendHandler }
        if (handler == null) {
            sendControlDirect(payload)
            return
        }

        val shouldPost = synchronized(sync) {
            pendingAbsoluteMovePayload = payload
            if (absoluteMoveDispatchQueued) {
                false
            } else {
                absoluteMoveDispatchQueued = true
                true
            }
        }
        if (!shouldPost) {
            return
        }

        handler.post {
            while (true) {
                val latestPayload = synchronized(sync) {
                    val current = pendingAbsoluteMovePayload
                    pendingAbsoluteMovePayload = null
                    current
                } ?: break
                sendControlDirect(latestPayload)
            }
            synchronized(sync) {
                absoluteMoveDispatchQueued = false
                if (pendingAbsoluteMovePayload != null && controlSendHandler != null) {
                    absoluteMoveDispatchQueued = true
                    controlSendHandler?.post {
                        while (true) {
                            val latestPayload = synchronized(sync) {
                                val current = pendingAbsoluteMovePayload
                                pendingAbsoluteMovePayload = null
                                current
                            } ?: break
                            sendControlDirect(latestPayload)
                        }
                        synchronized(sync) {
                            absoluteMoveDispatchQueued = false
                        }
                    }
                }
            }
        }
    }

    private fun sendControlDirect(payload: ByteArray) {
        val target = synchronized(sync) { relayRoute?.endpoint ?: remoteEndpoint } ?: return
        sendControlToTarget(payload, target)
    }

    private fun sendControlToTarget(payload: ByteArray, target: InetSocketAddress) {
        val packet = buildControlPacket(payload)
        val currentSocket = synchronized(sync) { socket } ?: return
        runCatching {
            val address = resolveAddress(target)
            if (address != null) {
                currentSocket.send(DatagramPacket(packet, packet.size, address, target.port))
            } else {
                Log.w("EVRT", "Dropping UDP control packet: unresolved target ${target.hostString}:${target.port}")
                mutateDiagnostics { current ->
                    current.copy(lastControlSendError = "РќРµ СѓРґР°Р»РѕСЃСЊ СЂР°Р·СЂРµС€РёС‚СЊ Р°РґСЂРµСЃ ${target.hostString}:${target.port}")
                }
                eventListener?.invoke("CONTROL SEND FAIL | unresolved=${target.hostString}:${target.port}")
            }
        }.onFailure { error ->
            Log.w("EVRT", "Failed to send UDP control packet to ${target.hostString}:${target.port}: ${error.message}", error)
            mutateDiagnostics { current ->
                current.copy(lastControlSendError = error.message ?: "UDP control send failed")
            }
            eventListener?.invoke("CONTROL SEND FAIL | target=${target.hostString}:${target.port} | error=${error.message ?: "unknown"}")
        }
    }

    private fun resolveAddress(endpoint: InetSocketAddress): java.net.InetAddress? {
        endpoint.address?.let { return it }
        return runCatching { java.net.InetAddress.getByName(endpoint.hostString) }.getOrNull()
    }

    private fun ensureControlSenderThread() {
        synchronized(sync) {
            if (controlSendThread != null && controlSendHandler != null) {
                return
            }

            val thread = HandlerThread("EvertyPcControlSender", Process.THREAD_PRIORITY_URGENT_DISPLAY)
            thread.start()
            controlSendThread = thread
            controlSendHandler = Handler(thread.looper)
            pendingAbsoluteMovePayload = null
            absoluteMoveDispatchQueued = false
        }
    }

    private fun updateRelayRegistrationLoop() {
        val runnableToRemove: Runnable?
        val handler: Handler?
        synchronized(sync) {
            runnableToRemove = relayRegisterRunnable
            handler = controlSendHandler
        }
        if (handler != null && runnableToRemove != null) {
            handler.removeCallbacks(runnableToRemove)
        }

        val shouldRun = synchronized(sync) {
            running && relayRegistrationRoute != null && controlSendHandler != null
        }
        if (!shouldRun) {
            synchronized(sync) {
                relayRegisterRunnable = null
            }
            return
        }

        val runnable = object : Runnable {
            override fun run() {
                sendRelayRegistration()
                synchronized(sync) {
                    if (relayRegisterRunnable === this && running && relayRegistrationRoute != null && controlSendHandler != null) {
                        controlSendHandler?.postDelayed(this, 2_000L)
                    }
                }
            }
        }

        synchronized(sync) {
            relayRegisterRunnable = runnable
            controlSendHandler?.post(runnable)
        }
    }

    private fun sendRelayRegistration() {
        val route = synchronized(sync) { relayRegistrationRoute } ?: return
        sendControlToTarget(buildRelayRegistration(route.sessionId, route.sessionToken, "receiver"), route.endpoint)
    }

    private fun mutateDiagnostics(transform: (PcReceiverDiagnostics) -> PcReceiverDiagnostics) {
        if (Looper.myLooper() == Looper.getMainLooper()) {
            diagnostics = transform(diagnostics)
        } else {
            mainHandler.post {
                diagnostics = transform(diagnostics)
            }
        }
    }

    private fun mutateState(transform: (PcReceiverUiState) -> PcReceiverUiState) {
        if (Looper.myLooper() == Looper.getMainLooper()) {
            uiState = transform(uiState)
        } else {
            mainHandler.post {
                uiState = transform(uiState)
            }
        }
    }

    private fun buildLocalAddressHint(port: Int): String {
        val ip = findLocalIpv4Address()
        return if (ip != null) "$ip:$port" else "РСЃРїРѕР»СЊР·СѓР№ LAN IP С‚РµР»РµС„РѕРЅР°:$port"
    }

    private fun findLocalIpv4Address(): String? {
        return runCatching {
            NetworkInterface.getNetworkInterfaces().toList()
                .asSequence()
                .filter { it.isUp && !it.isLoopback }
                .flatMap { it.inetAddresses.toList().asSequence() }
                .filterIsInstance<Inet4Address>()
                .firstOrNull { !it.isLoopbackAddress }
                ?.hostAddress
        }.getOrNull()
    }

    private fun buildCodecSpecificData(codec: String, codecConfig: ByteArray): List<ByteArray> {
        return when {
            codec.contains("av1", ignoreCase = true) -> emptyList()
            codec.contains("hevc", ignoreCase = true) -> {
                val hevcCsd = buildHevcCodecConfig(codecConfig)
                listOf(hevcCsd)
            }

            else -> {
                val (csd0, csd1) = splitAvcCodecConfig(codecConfig)
                listOf(csd0, csd1)
            }
        }
    }

    private fun splitAvcCodecConfig(codecConfig: ByteArray): Pair<ByteArray, ByteArray> {
        val nals = enumerateAnnexBNals(codecConfig, isHevc = false)
        val sps = nals.firstOrNull { (it.first.toInt() and 0x1F) == 7 }?.second?.let(::withStartCode)
            ?: throw IllegalArgumentException("Missing AVC SPS")
        val pps = nals.firstOrNull { (it.first.toInt() and 0x1F) == 8 }?.second?.let(::withStartCode)
            ?: throw IllegalArgumentException("Missing AVC PPS")
        return sps to pps
    }

    private fun buildHevcCodecConfig(codecConfig: ByteArray): ByteArray {
        val nals = enumerateAnnexBNals(codecConfig, isHevc = true)
        val units = nals.filter { (type, _) -> type.toInt() in 32..34 }
            .map { (_, nal) -> withStartCode(nal) }
        if (units.isEmpty()) {
            throw IllegalArgumentException("Missing HEVC VPS/SPS/PPS")
        }

        val totalSize = units.sumOf { it.size }
        return ByteArray(totalSize).also { combined ->
            var offset = 0
            for (unit in units) {
                System.arraycopy(unit, 0, combined, offset, unit.size)
                offset += unit.size
            }
        }
    }

    private fun withStartCode(nal: ByteArray): ByteArray {
        val startCode = byteArrayOf(0, 0, 0, 1)
        return ByteArray(startCode.size + nal.size).also { combined ->
            System.arraycopy(startCode, 0, combined, 0, startCode.size)
            System.arraycopy(nal, 0, combined, startCode.size, nal.size)
        }
    }

    private fun enumerateAnnexBNals(data: ByteArray, isHevc: Boolean): List<Pair<Byte, ByteArray>> {
        val result = mutableListOf<Pair<Byte, ByteArray>>()
        var offset = 0
        while (true) {
            val start = findStartCode(data, offset) ?: break
            val next = findStartCode(data, start.second) ?: Pair(data.size, 0)
            val nal = data.copyOfRange(start.second, next.first)
            if (nal.isNotEmpty()) {
                val nalType = if (isHevc) ((nal[0].toInt() shr 1) and 0x3F).toByte() else (nal[0].toInt() and 0x1F).toByte()
                result += nalType to nal
            }
            offset = next.first
        }
        return result
    }

    private fun findStartCode(data: ByteArray, offset: Int): Pair<Int, Int>? {
        var index = offset
        while (index <= data.size - 3) {
            if (data[index] == 0.toByte() && data[index + 1] == 0.toByte()) {
                if (data[index + 2] == 1.toByte()) {
                    return index to index + 3
                }
                if (index <= data.size - 4 && data[index + 2] == 0.toByte() && data[index + 3] == 1.toByte()) {
                    return index to index + 4
                }
            }
            index++
        }
        return null
    }

    private fun mapTouchToNormalized(
        x: Float,
        y: Float,
        viewWidth: Int,
        viewHeight: Int,
        videoWidth: Int,
        videoHeight: Int,
    ): Pair<Double, Double> {
        val scale = min(viewWidth / videoWidth.toDouble(), viewHeight / videoHeight.toDouble())
        val contentWidth = max(1, (videoWidth * scale).toInt())
        val contentHeight = max(1, (videoHeight * scale).toInt())
        val offsetX = (viewWidth - contentWidth) / 2f
        val offsetY = (viewHeight - contentHeight) / 2f
        val clampedX = x.coerceIn(offsetX, offsetX + contentWidth - 1f)
        val clampedY = y.coerceIn(offsetY, offsetY + contentHeight - 1f)
        val normalizedX = ((clampedX - offsetX) / max(1f, contentWidth - 1f)).toDouble()
        val normalizedY = ((clampedY - offsetY) / max(1f, contentHeight - 1f)).toDouble()
        return normalizedX to normalizedY
    }

    private fun buildControlPacket(payload: ByteArray): ByteArray {
        val buffer = ByteBuffer.allocate(TransportProtocol.HEADER_SIZE + payload.size).order(ByteOrder.BIG_ENDIAN)
        buffer.putInt(TransportProtocol.MAGIC)
        buffer.put(TransportProtocol.VERSION)
        buffer.put(TransportProtocol.TYPE_CONTROL)
        buffer.putShort(0)
        buffer.putInt(0)
        buffer.putShort(0)
        buffer.putShort(1)
        buffer.putLong(0L)
        buffer.put(payload)
        return buffer.array()
    }

    private fun buildRemoteMouseMoveAbsolute(seq: Long, x: Double, y: Double): ByteArray {
        return """{"kind":"remote_mouse_move_abs","seq":$seq,"x":$x,"y":$y}""".toByteArray(Charsets.UTF_8)
    }

    private fun buildRemoteMouseButton(seq: Long, button: String, pressed: Boolean): ByteArray {
        return """{"kind":"remote_mouse_button","seq":$seq,"button":"$button","pressed":$pressed}""".toByteArray(Charsets.UTF_8)
    }

    private fun buildRemoteMouseWheel(seq: Long, delta: Int): ByteArray {
        return """{"kind":"remote_mouse_wheel","seq":$seq,"delta":$delta}""".toByteArray(Charsets.UTF_8)
    }

    private fun buildRemoteKey(seq: Long, virtualKey: Int, pressed: Boolean): ByteArray {
        return """{"kind":"remote_key","seq":$seq,"vkey":$virtualKey,"pressed":$pressed}""".toByteArray(Charsets.UTF_8)
    }

    private fun buildRemoteGamepadState(
        seq: Long,
        controllerId: Int,
        buttons: UShort,
        leftTrigger: Int,
        rightTrigger: Int,
        leftThumbX: Int,
        leftThumbY: Int,
        rightThumbX: Int,
        rightThumbY: Int,
    ): ByteArray {
        return """{"kind":"remote_gamepad_state","seq":$seq,"controllerId":$controllerId,"buttons":${buttons.toInt()},"leftTrigger":$leftTrigger,"rightTrigger":$rightTrigger,"leftThumbX":$leftThumbX,"leftThumbY":$leftThumbY,"rightThumbX":$rightThumbX,"rightThumbY":$rightThumbY}"""
            .toByteArray(Charsets.UTF_8)
    }

    private fun buildRemoteReleaseAll(seq: Long): ByteArray {
        return """{"kind":"remote_release_all","seq":$seq}""".toByteArray(Charsets.UTF_8)
    }

    private fun buildRequestKeyFrame(): ByteArray {
        return """{"kind":"request_keyframe"}""".toByteArray(Charsets.UTF_8)
    }

    private fun buildReceiverStop(reason: String): ByteArray {
        return """{"kind":"receiver_stop","reason":"$reason"}""".toByteArray(Charsets.UTF_8)
    }

    private fun buildLatencyPulseRequest(seq: Long, source: String): ByteArray {
        return """{"kind":"latency_pulse_request","seq":$seq,"source":"$source"}""".toByteArray(Charsets.UTF_8)
    }

    private fun buildRelayRegistration(sessionId: String, sessionToken: String, role: String): ByteArray {
        return """{"kind":"relay_register","sessionId":"$sessionId","sessionToken":"$sessionToken","role":"$role"}"""
            .toByteArray(Charsets.UTF_8)
    }

    private fun buildReceiverPing(reason: String): ByteArray {
        return """{"kind":"receiver_ping","reason":"$reason"}""".toByteArray(Charsets.UTF_8)
    }

    private fun buildReceiverFeedback(
        pressure: String,
        backlogFrames: Int,
        queueDrops: Long,
        queueDropBurst: Long,
        decodeFps: Int,
        assemblyDelayMs: Int,
        arrivalDeltaMs: Int,
        decodeDeltaMs: Int,
        presentDeltaMs: Int,
        pulseEstimateMs: Int,
        inputEstimateMs: Int,
    ): ByteArray {
        return """{"kind":"receiver_feedback","pressure":"$pressure","backlogFrames":$backlogFrames,"queueDrops":$queueDrops,"queueDropBurst":$queueDropBurst,"decodeFps":$decodeFps,"assemblyDelayMs":$assemblyDelayMs,"arrivalDeltaMs":$arrivalDeltaMs,"decodeDeltaMs":$decodeDeltaMs,"presentDeltaMs":$presentDeltaMs,"pulseEstimateMs":$pulseEstimateMs,"inputEstimateMs":$inputEstimateMs}"""
            .toByteArray(Charsets.UTF_8)
    }

    private fun trimLatencyMapsLocked(nowNs: Long) {
        trimStaleLongEntries(frameArrivalNsByPts, nowNs)
        trimStaleLongEntries(framePresentedNsByPts, nowNs)
        trimStalePulseEntries(pendingLatencyPulses, nowNs)
        trimStaleLongEntries(latencyRequestStartedAtNsBySeq, nowNs)
    }

    private fun trimStaleLongEntries(entries: LinkedHashMap<Long, Long>, nowNs: Long) {
        val iterator = entries.entries.iterator()
        while (iterator.hasNext()) {
            val entry = iterator.next()
            if (entries.size > 48 || nowNs - entry.value > 5_000_000_000L) {
                iterator.remove()
            } else {
                break
            }
        }
    }

    private fun trimStalePulseEntries(entries: LinkedHashMap<Long, PendingLatencyPulse>, nowNs: Long) {
        val iterator = entries.entries.iterator()
        while (iterator.hasNext()) {
            val entry = iterator.next()
            if (entries.size > 32 || nowNs - entry.value.receivedAtNs > 5_000_000_000L) {
                iterator.remove()
            } else {
                break
            }
        }
    }

    private fun pushLatencyEstimate(target: ArrayDeque<Int>, value: Int) {
        if (value < 0) {
            return
        }

        target.addLast(value)
        while (target.size > 7) {
            target.removeFirst()
        }
    }

    private fun computeMedian(values: ArrayDeque<Int>): Int {
        if (values.isEmpty()) {
            return -1
        }

        val ordered = values.sorted()
        return ordered[ordered.size / 2]
    }

    private fun mapAndroidKeyCodeToWindowsVKey(keyCode: Int): Int? {
        return when (keyCode) {
            KeyEvent.KEYCODE_A -> 0x41
            KeyEvent.KEYCODE_B -> 0x42
            KeyEvent.KEYCODE_C -> 0x43
            KeyEvent.KEYCODE_D -> 0x44
            KeyEvent.KEYCODE_E -> 0x45
            KeyEvent.KEYCODE_F -> 0x46
            KeyEvent.KEYCODE_G -> 0x47
            KeyEvent.KEYCODE_H -> 0x48
            KeyEvent.KEYCODE_I -> 0x49
            KeyEvent.KEYCODE_J -> 0x4A
            KeyEvent.KEYCODE_K -> 0x4B
            KeyEvent.KEYCODE_L -> 0x4C
            KeyEvent.KEYCODE_M -> 0x4D
            KeyEvent.KEYCODE_N -> 0x4E
            KeyEvent.KEYCODE_O -> 0x4F
            KeyEvent.KEYCODE_P -> 0x50
            KeyEvent.KEYCODE_Q -> 0x51
            KeyEvent.KEYCODE_R -> 0x52
            KeyEvent.KEYCODE_S -> 0x53
            KeyEvent.KEYCODE_T -> 0x54
            KeyEvent.KEYCODE_U -> 0x55
            KeyEvent.KEYCODE_V -> 0x56
            KeyEvent.KEYCODE_W -> 0x57
            KeyEvent.KEYCODE_X -> 0x58
            KeyEvent.KEYCODE_Y -> 0x59
            KeyEvent.KEYCODE_Z -> 0x5A
            KeyEvent.KEYCODE_0 -> 0x30
            KeyEvent.KEYCODE_1 -> 0x31
            KeyEvent.KEYCODE_2 -> 0x32
            KeyEvent.KEYCODE_3 -> 0x33
            KeyEvent.KEYCODE_4 -> 0x34
            KeyEvent.KEYCODE_5 -> 0x35
            KeyEvent.KEYCODE_6 -> 0x36
            KeyEvent.KEYCODE_7 -> 0x37
            KeyEvent.KEYCODE_8 -> 0x38
            KeyEvent.KEYCODE_9 -> 0x39
            KeyEvent.KEYCODE_ENTER,
            KeyEvent.KEYCODE_NUMPAD_ENTER -> 0x0D
            KeyEvent.KEYCODE_DEL -> 0x08
            KeyEvent.KEYCODE_FORWARD_DEL -> 0x2E
            KeyEvent.KEYCODE_TAB -> 0x09
            KeyEvent.KEYCODE_SPACE -> 0x20
            KeyEvent.KEYCODE_ESCAPE -> 0x1B
            KeyEvent.KEYCODE_DPAD_LEFT -> 0x25
            KeyEvent.KEYCODE_DPAD_UP -> 0x26
            KeyEvent.KEYCODE_DPAD_RIGHT -> 0x27
            KeyEvent.KEYCODE_DPAD_DOWN -> 0x28
            KeyEvent.KEYCODE_PAGE_UP -> 0x21
            KeyEvent.KEYCODE_PAGE_DOWN -> 0x22
            KeyEvent.KEYCODE_MOVE_HOME -> 0x24
            KeyEvent.KEYCODE_MOVE_END -> 0x23
            KeyEvent.KEYCODE_INSERT -> 0x2D
            KeyEvent.KEYCODE_CTRL_LEFT,
            KeyEvent.KEYCODE_CTRL_RIGHT -> 0x11
            KeyEvent.KEYCODE_SHIFT_LEFT,
            KeyEvent.KEYCODE_SHIFT_RIGHT -> 0x10
            KeyEvent.KEYCODE_ALT_LEFT,
            KeyEvent.KEYCODE_ALT_RIGHT -> 0x12
            KeyEvent.KEYCODE_META_LEFT,
            KeyEvent.KEYCODE_META_RIGHT -> 0x5B
            KeyEvent.KEYCODE_CAPS_LOCK -> 0x14
            KeyEvent.KEYCODE_SCROLL_LOCK -> 0x91
            KeyEvent.KEYCODE_NUM_LOCK -> 0x90
            KeyEvent.KEYCODE_F1 -> 0x70
            KeyEvent.KEYCODE_F2 -> 0x71
            KeyEvent.KEYCODE_F3 -> 0x72
            KeyEvent.KEYCODE_F4 -> 0x73
            KeyEvent.KEYCODE_F5 -> 0x74
            KeyEvent.KEYCODE_F6 -> 0x75
            KeyEvent.KEYCODE_F7 -> 0x76
            KeyEvent.KEYCODE_F8 -> 0x77
            KeyEvent.KEYCODE_F9 -> 0x78
            KeyEvent.KEYCODE_F10 -> 0x79
            KeyEvent.KEYCODE_F11 -> 0x7A
            KeyEvent.KEYCODE_F12 -> 0x7B
            KeyEvent.KEYCODE_MINUS -> 0xBD
            KeyEvent.KEYCODE_EQUALS -> 0xBB
            KeyEvent.KEYCODE_LEFT_BRACKET -> 0xDB
            KeyEvent.KEYCODE_RIGHT_BRACKET -> 0xDD
            KeyEvent.KEYCODE_BACKSLASH -> 0xDC
            KeyEvent.KEYCODE_SEMICOLON -> 0xBA
            KeyEvent.KEYCODE_APOSTROPHE -> 0xDE
            KeyEvent.KEYCODE_COMMA -> 0xBC
            KeyEvent.KEYCODE_PERIOD -> 0xBE
            KeyEvent.KEYCODE_SLASH -> 0xBF
            KeyEvent.KEYCODE_GRAVE -> 0xC0
            else -> null
        }
    }

    private data class SessionConfig(
        val codec: String,
        val width: Int,
        val height: Int,
        val fps: Int,
        val preset: String,
    ) {
        val codecLabel: String
            get() = when {
                codec.contains("av1", ignoreCase = true) -> "AV1"
                codec.contains("hevc", ignoreCase = true) -> "H.265 / HEVC"
                else -> "H.264 / AVC"
            }
        val isGame: Boolean
            get() = preset.equals("GAME", ignoreCase = true)

        companion object {
            fun parse(payload: ByteArray): SessionConfig? {
                return runCatching {
                    val text = payload.toString(Charsets.UTF_8)
                    val json = org.json.JSONObject(text)
                    val codec = json.optString("codec", "video/avc")
                    val width = json.optInt("baseWidth", json.optInt("width", 0))
                    val height = json.optInt("baseHeight", json.optInt("height", 0))
                    val fps = json.optInt("fps", 60).coerceAtLeast(1)
                    val preset = json.optString("preset", "MEDIA")
                    if (width <= 0 || height <= 0) null else SessionConfig(codec = codec, width = width, height = height, fps = fps, preset = preset)
                }.getOrNull()
            }
        }
    }

    internal data class ReceiverFeedbackSnapshot(
        val pressure: String,
        val decodeFps: Int,
        val queueDrops: Long,
        val queueDropBurst: Long,
        val decodeDeltaMs: Int,
        val presentDeltaMs: Int,
        val pulseEstimateMs: Int,
        val inputEstimateMs: Int,
    )

    fun setReceiverFeedbackListener(listener: ((ReceiverFeedbackSnapshot) -> Unit)?) {
        receiverFeedbackListener = listener
    }

    fun setEventListener(listener: ((String) -> Unit)?) {
        eventListener = listener
    }

    private data class PendingLatencyPulse(
        val pulseId: Long,
        val source: String,
        val presentationTimeUs: Long,
        val senderPipelineMs: Int,
        val approxSenderMs: Int,
        val inputSeq: Long,
        val receivedAtNs: Long = System.nanoTime(),
    )

    private data class DecoderConfig(
        val width: Int,
        val height: Int,
        val codec: String,
        val codecConfigHash: Int,
    )

    internal data class EvrtPacket(
        val type: Byte,
        val flags: Int,
        val frameId: Int,
        val packetIndex: Int,
        val packetCount: Int,
        val presentationTimeUs: Long,
        val payload: ByteArray,
    ) {
        companion object {
            fun parse(buffer: ByteArray, length: Int): EvrtPacket? {
                if (length < TransportProtocol.HEADER_SIZE) {
                    return null
                }
                val header = ByteBuffer.wrap(buffer, 0, length).order(ByteOrder.BIG_ENDIAN)
                if (header.int != TransportProtocol.MAGIC) {
                    return null
                }
                val version = header.get()
                if (version.toInt() < 2) {
                    return null
                }
                val type = header.get()
                val flags = header.short.toInt() and 0xFFFF
                val frameId = header.int
                val packetIndex = header.short.toInt() and 0xFFFF
                val packetCount = header.short.toInt() and 0xFFFF
                val presentationTimeUs = header.long
                val payload = buffer.copyOfRange(TransportProtocol.HEADER_SIZE, length)
                return EvrtPacket(type, flags, frameId, packetIndex, packetCount, presentationTimeUs, payload)
            }
        }
    }

    private class AccessUnitAssembler(
        private val onFrameReady: (ByteArray, Long, Boolean) -> Unit,
        private val onFrameDropped: () -> Unit,
    ) {
        private var currentFrameId: Int = -1
        private var currentPacketCount: Int = 0
        private var currentPresentationTimeUs: Long = 0L
        private var currentIsKeyFrame = false
        private var fragments: Array<ByteArray?> = emptyArray()
        private var receivedCount = 0

        fun push(packet: EvrtPacket) {
            if (packet.packetCount <= 0 || packet.packetIndex >= packet.packetCount) {
                return
            }

            if (currentFrameId != packet.frameId) {
                if (currentFrameId >= 0 && receivedCount < currentPacketCount && packet.frameId > currentFrameId) {
                    onFrameDropped()
                }
                currentFrameId = packet.frameId
                currentPacketCount = packet.packetCount
                currentPresentationTimeUs = packet.presentationTimeUs
                currentIsKeyFrame = (packet.flags and 0x0001) != 0
                fragments = arrayOfNulls(packet.packetCount)
                receivedCount = 0
            }

            if (packet.packetCount != currentPacketCount || packet.frameId != currentFrameId) {
                return
            }

            if (fragments[packet.packetIndex] == null) {
                fragments[packet.packetIndex] = packet.payload
                receivedCount++
            }

            if (receivedCount == currentPacketCount) {
                val totalBytes = fragments.sumOf { it?.size ?: 0 }
                val combined = ByteArray(totalBytes)
                var offset = 0
                for (fragment in fragments) {
                    val bytes = fragment ?: return
                    System.arraycopy(bytes, 0, combined, offset, bytes.size)
                    offset += bytes.size
                }

                onFrameReady(combined, currentPresentationTimeUs, currentIsKeyFrame)
                reset()
            }
        }

        fun reset() {
            currentFrameId = -1
            currentPacketCount = 0
            currentPresentationTimeUs = 0L
            currentIsKeyFrame = false
            fragments = emptyArray()
            receivedCount = 0
        }
    }
}
