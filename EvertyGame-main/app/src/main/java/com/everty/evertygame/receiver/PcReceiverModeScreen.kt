package com.everty.evertygame.receiver

import android.app.Activity
import android.content.Context
import android.content.ContextWrapper
import android.graphics.SurfaceTexture
import android.media.MediaCodecList
import android.os.Build
import android.os.SystemClock
import android.app.UiModeManager
import android.content.pm.PackageManager
import android.content.res.Configuration
import android.view.GestureDetector
import android.view.MotionEvent
import android.view.ScaleGestureDetector
import android.view.Surface
import android.view.SurfaceHolder
import android.view.SurfaceView
import android.view.TextureView
import android.view.View
import android.view.InputDevice
import android.view.KeyEvent
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.focusable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.ElevatedCard
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusProperties
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import android.util.Log
import java.net.Inet4Address
import java.net.NetworkInterface
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

private const val LegacyLocalControlPlaneUrl = "http://192.168.0.5:5180"
private const val DefaultControlPlaneUrl = "http://46.45.217.19:5180"
private const val DefaultRelayHost = "46.45.217.19"
private const val DefaultRelayPort = 6200

private enum class PcDesiredStreamPreset(
    val title: String,
    val summary: String,
    val width: Int,
    val height: Int,
    val fps: Int,
    val bitrateMbps: Double,
    val preferredCodecs: List<String>,
) {
    LOW_LATENCY(
        title = "Low Latency",
        summary = "720p / 60 / 6.5",
        width = 1280,
        height = 720,
        fps = 60,
        bitrateMbps = 6.5,
        preferredCodecs = listOf("video/avc", "video/hevc", "video/av1"),
    ),
    BALANCED(
        title = "Balanced",
        summary = "1080p / 60 / 8.5",
        width = 1920,
        height = 1080,
        fps = 60,
        bitrateMbps = 8.5,
        preferredCodecs = listOf("video/hevc", "video/av1", "video/avc"),
    ),
    QUALITY(
        title = "Quality",
        summary = "1440p / 60 / 14",
        width = 2560,
        height = 1440,
        fps = 60,
        bitrateMbps = 14.0,
        preferredCodecs = listOf("video/hevc", "video/av1", "video/avc"),
    ),
    AV1_MAX(
        title = "AV1 Max",
        summary = "1440p / 90 / 12",
        width = 2560,
        height = 1440,
        fps = 90,
        bitrateMbps = 12.0,
        preferredCodecs = listOf("video/av1", "video/hevc", "video/avc"),
    ),
}

private enum class MinimalReceiverScreenState {
    Idle,
    Connecting,
    RegisteringRelay,
    WaitingForStream,
    Streaming,
    Stopping,
    Error,
}

private class PreviewTransformState {
    var scale by mutableStateOf(1f)
    var translationX by mutableStateOf(0f)
    var translationY by mutableStateOf(0f)
    var viewportWidth by mutableStateOf(0)
    var viewportHeight by mutableStateOf(0)

    fun updateViewport(width: Int, height: Int) {
        viewportWidth = width
        viewportHeight = height
        clampToViewport()
    }

    fun applyScale(scaleFactor: Float, focusX: Float, focusY: Float) {
        val oldScale = scale
        val newScale = (oldScale * scaleFactor).coerceIn(1f, 4f)
        if (kotlin.math.abs(newScale - oldScale) < 0.0001f) {
            return
        }

        val ratio = newScale / oldScale
        translationX = focusX - (focusX - translationX) * ratio
        translationY = focusY - (focusY - translationY) * ratio
        scale = newScale
        clampToViewport()
    }

    fun panBy(deltaX: Float, deltaY: Float) {
        if (!isZoomed) {
            return
        }
        translationX += deltaX
        translationY += deltaY
        clampToViewport()
    }

    fun reset() {
        scale = 1f
        translationX = 0f
        translationY = 0f
    }

    val isZoomed: Boolean
        get() = scale > 1.01f

    private fun clampToViewport() {
        if (viewportWidth <= 0 || viewportHeight <= 0 || scale <= 1f) {
            translationX = 0f
            translationY = 0f
            if (scale < 1f) {
                scale = 1f
            }
            return
        }

        val maxOffsetX = (viewportWidth * (scale - 1f)) / 2f
        val maxOffsetY = (viewportHeight * (scale - 1f)) / 2f
        translationX = translationX.coerceIn(-maxOffsetX, maxOffsetX)
        translationY = translationY.coerceIn(-maxOffsetY, maxOffsetY)
    }
}

private fun Context.findActivity(): Activity? = when (this) {
    is Activity -> this
    is ContextWrapper -> baseContext.findActivity()
    else -> null
}

@Composable
private fun ReceiverImmersiveMode(enabled: Boolean) {
    val view = LocalView.current
    val activity = LocalContext.current.findActivity()

    DisposableEffect(activity, view, enabled) {
        val window = activity?.window
        val insetsController =
            if (window != null) {
                WindowCompat.setDecorFitsSystemWindows(window, !enabled)
                WindowInsetsControllerCompat(window, view)
            } else {
                null
            }

        if (enabled) {
            insetsController?.systemBarsBehavior =
                WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
            insetsController?.hide(WindowInsetsCompat.Type.systemBars())
        } else {
            insetsController?.show(WindowInsetsCompat.Type.systemBars())
        }

        onDispose {
            if (window != null) {
                WindowCompat.setDecorFitsSystemWindows(window, true)
            }
            insetsController?.show(WindowInsetsCompat.Type.systemBars())
        }
    }

    SideEffect {
        val window = activity?.window ?: return@SideEffect
        WindowCompat.setDecorFitsSystemWindows(window, !enabled)
    }
}

private suspend fun awaitUsableConnectInstructions(
    controlPlaneClient: ControlPlaneClient,
    baseUrl: String,
    sessionId: String,
    sessionToken: String,
): ControlPlaneConnectInstructions {
    var lastConnect: ControlPlaneConnectInstructions? = null
    repeat(8) {
        val connect = controlPlaneClient.getConnectInstructions(
            baseUrl = baseUrl,
            sessionId = sessionId,
            sessionToken = sessionToken,
        )
        lastConnect = connect
        val relayReady = !connect.relayHost.isNullOrBlank() && connect.relayPort != null && connect.relayPort in 1..65535
        if (relayReady) {
            return connect
        }
        delay(500L)
    }

    return lastConnect ?: controlPlaneClient.getConnectInstructions(
        baseUrl = baseUrl,
        sessionId = sessionId,
        sessionToken = sessionToken,
    )
}

private suspend fun awaitHostReady(
    controlPlaneClient: ControlPlaneClient,
    baseUrl: String,
    sessionId: String,
    sessionToken: String,
): ControlPlaneConnectInstructions {
    var lastConnect: ControlPlaneConnectInstructions? = null
    repeat(12) {
        val connect = controlPlaneClient.getConnectInstructions(
            baseUrl = baseUrl,
            sessionId = sessionId,
            sessionToken = sessionToken,
        )
        lastConnect = connect
        if (connect.hostReady) {
            return connect
        }
        delay(500L)
    }

    return lastConnect ?: controlPlaneClient.getConnectInstructions(
        baseUrl = baseUrl,
        sessionId = sessionId,
        sessionToken = sessionToken,
    )
}

private suspend fun awaitReceiverRegistered(
    controlPlaneClient: ControlPlaneClient,
    baseUrl: String,
    sessionId: String,
    sessionToken: String,
): ControlPlaneConnectInstructions {
    var lastConnect: ControlPlaneConnectInstructions? = null
    repeat(12) {
        val connect = controlPlaneClient.getConnectInstructions(
            baseUrl = baseUrl,
            sessionId = sessionId,
            sessionToken = sessionToken,
        )
        lastConnect = connect
        if (connect.receiverRegistered) {
            return connect
        }
        delay(250L)
    }

    return lastConnect ?: controlPlaneClient.getConnectInstructions(
        baseUrl = baseUrl,
        sessionId = sessionId,
        sessionToken = sessionToken,
    )
}

private suspend fun awaitSessionStopped(
    controlPlaneClient: ControlPlaneClient,
    baseUrl: String,
    sessionId: String,
    sessionToken: String,
): Boolean {
    repeat(20) {
        val stillActive = runCatching {
            val connect = controlPlaneClient.getConnectInstructions(
                baseUrl = baseUrl,
                sessionId = sessionId,
                sessionToken = sessionToken,
            )
            connect.status.equals("Active", ignoreCase = true) ||
                connect.status.equals("Pending", ignoreCase = true)
        }.getOrElse {
            false
        }
        if (!stillActive) {
            return true
        }
        delay(250L)
    }
    return false
}

private fun fallbackRelayHost(host: String?): String = host?.trim().orEmpty().ifBlank { DefaultRelayHost }

private fun fallbackRelayPort(port: Int?): Int = port?.takeIf { it in 1..65535 } ?: DefaultRelayPort

private fun detectSupportedDecodeCodecs(): List<String> {
    val codecInfos = MediaCodecList(MediaCodecList.REGULAR_CODECS).codecInfos
    val supported = linkedSetOf<String>()
    listOf("video/av1", "video/hevc", "video/avc").forEach { mime ->
        val hasDecoder = codecInfos.any { info ->
            !info.isEncoder && info.supportedTypes.any { it.equals(mime, ignoreCase = true) }
        }
        if (hasDecoder) {
            supported += mime
        }
    }
    return if (supported.isEmpty()) listOf("video/avc") else supported.toList()
}

private fun detectLanIpv4Addresses(): List<String> {
    return runCatching {
        NetworkInterface.getNetworkInterfaces().toList()
            .asSequence()
            .filter { it.isUp && !it.isLoopback && !it.isVirtual }
            .flatMap { iface -> iface.inetAddresses.toList().asSequence() }
            .filterIsInstance<Inet4Address>()
            .map { it.hostAddress }
            .filterNotNull()
            .filter { candidate ->
                candidate.startsWith("10.") ||
                    candidate.startsWith("192.168.") ||
                    candidate.startsWith("172.16.") ||
                    candidate.startsWith("172.17.") ||
                    candidate.startsWith("172.18.") ||
                    candidate.startsWith("172.19.") ||
                    candidate.startsWith("172.20.") ||
                    candidate.startsWith("172.21.") ||
                    candidate.startsWith("172.22.") ||
                    candidate.startsWith("172.23.") ||
                    candidate.startsWith("172.24.") ||
                    candidate.startsWith("172.25.") ||
                    candidate.startsWith("172.26.") ||
                    candidate.startsWith("172.27.") ||
                    candidate.startsWith("172.28.") ||
                    candidate.startsWith("172.29.") ||
                    candidate.startsWith("172.30.") ||
                    candidate.startsWith("172.31.")
            }
            .distinct()
            .toList()
    }.getOrDefault(emptyList())
}

private fun isAndroidTvDevice(context: Context): Boolean {
    val packageManager = context.packageManager
    if (packageManager.hasSystemFeature(PackageManager.FEATURE_LEANBACK) ||
        packageManager.hasSystemFeature(PackageManager.FEATURE_LEANBACK_ONLY)
    ) {
        return true
    }

    val uiModeManager = context.getSystemService(UiModeManager::class.java)
    return uiModeManager?.currentModeType == Configuration.UI_MODE_TYPE_TELEVISION
}

private fun applyPreset(
    preset: PcDesiredStreamPreset,
    setWidth: (String) -> Unit,
    setHeight: (String) -> Unit,
    setFps: (String) -> Unit,
    setBitrate: (String) -> Unit,
    setPreferHevc: (Boolean) -> Unit,
) {
    setWidth(preset.width.toString())
    setHeight(preset.height.toString())
    setFps(preset.fps.toString())
    setBitrate(preset.bitrateMbps.toString())
    setPreferHevc(!preset.preferredCodecs.first().contains("avc", ignoreCase = true))
}

private suspend fun probeRelayTcp(host: String, port: Int): Pair<Boolean, String> {
    return withContext(Dispatchers.IO) {
        runCatching {
            java.net.Socket().use { socket ->
                socket.soTimeout = 1500
                socket.connect(java.net.InetSocketAddress(host, port), 1500)
            }
            true to "ok"
        }.getOrElse { false to (it.message ?: "error") }
    }
}

@Composable
fun PcReceiverModeScreen(
    onFullscreenChanged: (Boolean) -> Unit = {},
) {
    val context = LocalContext.current
    val controller = remember { PcReceiverClientController(context.applicationContext) }
    val controlPlaneClient = remember(context) { ControlPlaneClient(context.applicationContext) }
    val scope = rememberCoroutineScope()

    var controlPlaneUrl by rememberSaveable { mutableStateOf(DefaultControlPlaneUrl) }
    var pcCode by rememberSaveable { mutableStateOf("") }
    var listenPortText by rememberSaveable { mutableStateOf("5001") }
    var clientRegionText by rememberSaveable { mutableStateOf("global") }
    var requestedWidthText by rememberSaveable { mutableStateOf("1280") }
    var requestedHeightText by rememberSaveable { mutableStateOf("720") }
    var requestedFpsText by rememberSaveable { mutableStateOf("60") }
    var requestedBitrateText by rememberSaveable { mutableStateOf("8.5") }
    var selectedPresetName by rememberSaveable { mutableStateOf(PcDesiredStreamPreset.BALANCED.name) }
    var preferHevc by rememberSaveable { mutableStateOf(true) }
    var preferNativeDecoder by rememberSaveable { mutableStateOf(true) }
    var preferRelay by rememberSaveable { mutableStateOf(true) }
    var requestAudio by rememberSaveable { mutableStateOf(false) }
    var settingsVisible by rememberSaveable { mutableStateOf(false) }
    var diagnosticsVisible by rememberSaveable { mutableStateOf(false) }
    var logsVisible by rememberSaveable { mutableStateOf(false) }

    var screenState by rememberSaveable { mutableStateOf(MinimalReceiverScreenState.Idle) }
    var statusText by rememberSaveable { mutableStateOf("Ready") }
    var errorText by rememberSaveable { mutableStateOf<String?>(null) }
    var connectionAttemptId by rememberSaveable { mutableStateOf(0L) }

    var activeSessionId by rememberSaveable { mutableStateOf("") }
    var activeSessionToken by rememberSaveable { mutableStateOf("") }
    var activeHostLabel by rememberSaveable { mutableStateOf("PC") }
    var activeRouteKind by rememberSaveable { mutableStateOf("-") }
    var hostReady by rememberSaveable { mutableStateOf(false) }
    var lastStoppedSessionId by rememberSaveable { mutableStateOf("") }
    val eventLogs = remember { mutableStateListOf<String>() }
    val supportedDecodeCodecs = remember { detectSupportedDecodeCodecs() }
    val lanAddresses = remember { detectLanIpv4Addresses() }
    val selectedPreset = remember(selectedPresetName) {
        runCatching { PcDesiredStreamPreset.valueOf(selectedPresetName) }.getOrDefault(PcDesiredStreamPreset.BALANCED)
    }

    val uiState = controller.uiState
    val diagnostics = controller.diagnostics
    val viewerVisible = activeSessionId.isNotBlank() || screenState in setOf(
        MinimalReceiverScreenState.WaitingForStream,
        MinimalReceiverScreenState.Streaming,
        MinimalReceiverScreenState.Stopping,
    )
    val previewAspectRatio =
        if (uiState.metrics.videoWidth > 0 && uiState.metrics.videoHeight > 0) {
            uiState.metrics.videoWidth.toFloat() / uiState.metrics.videoHeight.toFloat()
        } else {
            16f / 9f
        }

    fun nextAttemptId(): Long {
        connectionAttemptId += 1L
        return connectionAttemptId
    }

    fun appendEventLog(message: String) {
        val line = "${System.currentTimeMillis()} | $message"
        Log.d("EVRT", line)
        eventLogs.add(line)
        if (eventLogs.size > 200) {
            eventLogs.removeRange(0, eventLogs.size - 200)
        }
    }

    val logSink by rememberUpdatedState<(String) -> Unit>({ message -> appendEventLog(message) })

    LaunchedEffect(Unit) {
        if (controlPlaneUrl.trim() == LegacyLocalControlPlaneUrl) {
            controlPlaneUrl = DefaultControlPlaneUrl
            appendEventLog("CONTROL PLANE URL MIGRATED | from=$LegacyLocalControlPlaneUrl | to=$DefaultControlPlaneUrl")
        }
    }

    LaunchedEffect(selectedPresetName) {
        applyPreset(
            preset = selectedPreset,
            setWidth = { requestedWidthText = it },
            setHeight = { requestedHeightText = it },
            setFps = { requestedFpsText = it },
            setBitrate = { requestedBitrateText = it },
            setPreferHevc = { preferHevc = it },
        )
    }

    DisposableEffect(controller) {
        controller.setEventListener { message -> logSink(message) }
        PcReceiverClientController.setActiveController(controller)
        onDispose {
            controller.setEventListener(null)
            if (PcReceiverClientController.activeController() === controller) {
                PcReceiverClientController.setActiveController(null)
            }
        }
    }

    LaunchedEffect(preferNativeDecoder) {
        controller.setPreferNativeDecoder(preferNativeDecoder)
    }

    fun isAttemptCurrent(attemptId: Long): Boolean = connectionAttemptId == attemptId

    fun resetActiveState() {
        activeSessionId = ""
        activeSessionToken = ""
        activeHostLabel = "PC"
        activeRouteKind = "-"
        hostReady = false
    }

    fun dumpConnectionFailure(
        attemptId: Long,
        phase: String,
        throwable: Throwable,
        sessionId: String,
        sessionToken: String,
        baseUrl: String,
    ) {
        val diag = controller.diagnostics
        val ui = controller.uiState
        Log.e(
            "EVRT",
            buildString {
                append("CONNECTION FAILURE | ")
                append("attempt=").append(attemptId).append(" | ")
                append("phase=").append(phase).append(" | ")
                append("error=").append(throwable.javaClass.simpleName).append(": ").append(throwable.message ?: "-").append(" | ")
                append("sessionId=").append(sessionId.ifBlank { "-" }).append(" | ")
                append("sessionToken=").append(if (sessionToken.isBlank()) "-" else "***").append(" | ")
                append("baseUrl=").append(baseUrl.ifBlank { "-" }).append(" | ")
                append("screenState=").append(screenState).append(" | ")
                append("uiPhase=").append(ui.phase).append(" | ")
                append("uiStatus=").append(ui.status).append(" | ")
                append("diag.sessionId=").append(diag.sessionId).append(" | ")
                append("diag.route=").append(diag.routeKind).append(" | ")
                append("diag.relay=").append(diag.relayEndpoint).append(" | ")
                append("diag.receiverRegistered=").append(diag.receiverRegistered).append(" | ")
                append("diag.lastRelayAck=").append(diag.lastRelayAck).append(" | ")
                append("diag.firstPacket=").append(diag.firstPacketReceived).append(" | ")
                append("diag.lastMediaPacket=").append(diag.lastMediaPacketFrom).append(" | ")
                append("diag.lastControlError=").append(diag.lastControlSendError)
            },
            throwable,
        )
    }

    fun stopSession(reason: String) {
        scope.launch {
            val attemptId = nextAttemptId()
            val baseUrl = controlPlaneUrl.trim()
            val sessionId = activeSessionId
            val sessionToken = activeSessionToken
            appendEventLog("STOP FLOW BEGIN | attempt=$attemptId | reason=$reason | activeSessionId=${sessionId.ifBlank { "-" }} | screenState=$screenState | uiPhase=${uiState.phase} | diagSession=${diagnostics.sessionId}")

            screenState = MinimalReceiverScreenState.Stopping
            statusText = "Disconnecting"
            errorText = null

            controller.markStopping()
            controller.sendReceiverStop(reason)
            delay(250L)
            controller.configureManagedRelayRoute(null, null, null, null)
            controller.stop()
            appendEventLog("STOP FLOW LOCAL STOPPED | attempt=$attemptId | priorSessionId=${sessionId.ifBlank { "-" }} | diagSession=${controller.diagnostics.sessionId} | firstPacket=${controller.diagnostics.firstPacketReceived}")
            if (sessionId.isNotBlank()) {
                lastStoppedSessionId = sessionId
            }
            resetActiveState()
            appendEventLog("STOP FLOW STATE CLEARED | attempt=$attemptId | activeSessionId=${activeSessionId.ifBlank { "-" }} | lastStoppedSessionId=${lastStoppedSessionId.ifBlank { "-" }}")

            try {
                if (sessionId.isNotBlank() && sessionToken.isNotBlank() && baseUrl.isNotBlank()) {
                    appendEventLog("STOP SESSION | attempt=$attemptId | sessionId=$sessionId | reason=$reason")
                    controlPlaneClient.stopSession(
                        baseUrl = baseUrl,
                        sessionId = sessionId,
                        sessionToken = sessionToken,
                        reason = reason,
                    )
                    val stoppedConfirmed = awaitSessionStopped(
                        controlPlaneClient = controlPlaneClient,
                        baseUrl = baseUrl,
                        sessionId = sessionId,
                        sessionToken = sessionToken,
                    )
                    appendEventLog("STOP SESSION CONFIRM | attempt=$attemptId | sessionId=$sessionId | stopped=$stoppedConfirmed")
                }
                runCatching { controlPlaneClient.clearAllManagedSessionState() }
                appendEventLog("STOP FLOW MANAGED STATE CLEARED | attempt=$attemptId | stoppedSessionId=${sessionId.ifBlank { "-" }}")
                delay(350L)
            } finally {
                if (isAttemptCurrent(attemptId)) {
                    screenState = MinimalReceiverScreenState.Idle
                    statusText = "Disconnected"
                    errorText = null
                    appendEventLog("STOP FLOW END | attempt=$attemptId | screenState=$screenState | activeSessionId=${activeSessionId.ifBlank { "-" }} | lastStoppedSessionId=${lastStoppedSessionId.ifBlank { "-" }}")
                }
            }
        }
    }

    fun connect() {
        val baseUrl = controlPlaneUrl.trim()
        val hostId = pcCode.trim().lowercase()
        val port = listenPortText.toIntOrNull()

        if (baseUrl.isBlank()) {
            screenState = MinimalReceiverScreenState.Error
            statusText = "Connection failed"
            errorText = "Control plane URL is empty."
            return
        }
        if (hostId.length != 4) {
            screenState = MinimalReceiverScreenState.Error
            statusText = "Connection failed"
            errorText = "Enter a valid 4-character PC code."
            return
        }
        if (port == null || port !in 1..65535) {
            screenState = MinimalReceiverScreenState.Error
            statusText = "Connection failed"
            errorText = "Enter a valid UDP port."
            return
        }

        scope.launch {
            val attemptId = nextAttemptId()
            var createdSessionId = ""
            var createdSessionToken = ""

            try {
                runCatching {
                    if (activeSessionId.isNotBlank() && activeSessionToken.isNotBlank()) {
                        appendEventLog("REPLACE SESSION | attempt=$attemptId | previousSessionId=$activeSessionId")
                        controlPlaneClient.stopSession(
                            baseUrl = baseUrl,
                            sessionId = activeSessionId,
                            sessionToken = activeSessionToken,
                            reason = "android_replace_session",
                        )
                    }
                }
                runCatching { controlPlaneClient.clearAllManagedSessionState() }
                appendEventLog("CONNECT BEGIN | attempt=$attemptId | hostId=$hostId | baseUrl=$baseUrl | lastStoppedSessionId=${lastStoppedSessionId.ifBlank { "-" }}")

                screenState = MinimalReceiverScreenState.Connecting
                statusText = "Connecting to PC"
                errorText = null
                resetActiveState()

                controller.configureManagedRelayRoute(null, null, null, null)
                controller.stop()
                controller.start(port)

                val endpoint = controller.resolveReceiverEndpoint(port)
                    ?: throw IllegalStateException("Phone UDP endpoint is unavailable.")
                val preferredCodecs = selectedPreset.preferredCodecs
                    .filter { codec -> supportedDecodeCodecs.any { it.equals(codec, ignoreCase = true) } }
                    .ifEmpty {
                        listOf(
                            if (preferHevc && supportedDecodeCodecs.any { it.equals("video/hevc", ignoreCase = true) }) "video/hevc" else "video/avc",
                        )
                    }

                val session = controlPlaneClient.createSession(
                    baseUrl = baseUrl,
                    hostId = hostId,
                    clientLabel = "android-pc-receiver",
                    clientRegion = clientRegionText.trim().ifBlank { "global" },
                    codecPreference = preferredCodecs.firstOrNull() ?: if (preferHevc) "video/hevc" else "video/avc",
                    preferRelay = preferRelay,
                    audioRequested = requestAudio,
                    controllerCount = 1,
                    leaseMinutes = 30,
                    receiverAddress = endpoint.first,
                    receiverPort = endpoint.second,
                    desiredStream = ControlPlaneDesiredStreamRequest(
                        width = parseOptionalInt(requestedWidthText),
                        height = parseOptionalInt(requestedHeightText),
                        fps = parseOptionalInt(requestedFpsText),
                        bitrateBps = parseBitrateMbpsToBps(requestedBitrateText),
                        captureCursor = false,
                        adaptiveMode = false,
                        preferredCodecs = preferredCodecs,
                        presetId = selectedPreset.name,
                    ),
                    clientCapabilities = ControlPlaneClientCapabilities(
                        supportedDecodeCodecs = supportedDecodeCodecs,
                        lanAddresses = (lanAddresses + endpoint.first).distinct(),
                    ),
                )
                createdSessionId = session.sessionId
                createdSessionToken = session.sessionToken
                appendEventLog("CREATE SESSION | attempt=$attemptId | sessionId=${session.sessionId} | hostId=$hostId")
                if (lastStoppedSessionId.isNotBlank() && session.sessionId == lastStoppedSessionId) {
                    throw IllegalStateException("Server returned the previously stopped session again: ${session.sessionId}")
                }

                if (!isAttemptCurrent(attemptId)) {
                    throw IllegalStateException("Connection superseded.")
                }

                activeSessionId = session.sessionId
                activeSessionToken = session.sessionToken
                activeHostLabel = session.hostDisplayName
                lastStoppedSessionId = ""

                runCatching {
                    controlPlaneClient.publishNatProbe(
                        baseUrl = baseUrl,
                        sessionId = session.sessionId,
                        sessionToken = session.sessionToken,
                        probeToken = session.probeToken,
                        probeHost = session.probeAddress.orEmpty(),
                        probePort = session.probePort ?: 0,
                        role = "client",
                    )
                }

                val connectInstructions = awaitUsableConnectInstructions(
                    controlPlaneClient = controlPlaneClient,
                    baseUrl = baseUrl,
                    sessionId = session.sessionId,
                    sessionToken = session.sessionToken,
                )

                if (!isAttemptCurrent(attemptId)) {
                    throw IllegalStateException("Connection superseded.")
                }

                activeRouteKind = connectInstructions.routeKind
                hostReady = connectInstructions.hostReady
                controller.updateManagedDiagnostics(
                    sessionId = session.sessionId,
                    routeKind = connectInstructions.routeKind,
                    relayHost = connectInstructions.relayHost,
                    relayPort = connectInstructions.relayPort,
                )
                val relayHost = fallbackRelayHost(connectInstructions.relayHost)
                val relayPort = fallbackRelayPort(connectInstructions.relayPort)
                if (connectInstructions.routeKind.contains("relay", ignoreCase = true)) {
                    val (ok, detail) = probeRelayTcp(relayHost, relayPort)
                    appendEventLog("RELAY TCP PROBE | target=$relayHost:$relayPort | ok=$ok | detail=$detail")
                }
                controller.configureManagedRelayRegistration(
                    sessionId = session.sessionId,
                    sessionToken = session.sessionToken,
                    relayHost = relayHost,
                    relayPort = relayPort,
                )
                controller.configureManagedRelayRoute(
                    sessionId = session.sessionId,
                    sessionToken = session.sessionToken,
                    relayHost = relayHost.takeIf { connectInstructions.routeKind.contains("relay", ignoreCase = true) },
                    relayPort = relayPort.takeIf { connectInstructions.routeKind.contains("relay", ignoreCase = true) },
                )

                screenState = MinimalReceiverScreenState.RegisteringRelay
                statusText = "Registering phone"
                controller.markRegisteringRelay()
                controller.armRelayRegistrationBurst("connect")

                val registrationSnapshot = awaitReceiverRegistered(
                    controlPlaneClient = controlPlaneClient,
                    baseUrl = baseUrl,
                    sessionId = session.sessionId,
                    sessionToken = session.sessionToken,
                )
                var relayRegistered = registrationSnapshot.receiverRegistered
                if (!relayRegistered && connectInstructions.routeKind.contains("relay", ignoreCase = true)) {
                    appendEventLog("RELAY REGISTER RETRY | attempt=$attemptId | action=restart_local_listener")
                    controller.stop()
                    controller.start(port)
                    controller.updateManagedDiagnostics(
                        sessionId = session.sessionId,
                        routeKind = connectInstructions.routeKind,
                        relayHost = relayHost,
                        relayPort = relayPort,
                    )
                    controller.configureManagedRelayRegistration(
                        sessionId = session.sessionId,
                        sessionToken = session.sessionToken,
                        relayHost = relayHost,
                        relayPort = relayPort,
                    )
                    controller.configureManagedRelayRoute(
                        sessionId = session.sessionId,
                        sessionToken = session.sessionToken,
                        relayHost = relayHost.takeIf { connectInstructions.routeKind.contains("relay", ignoreCase = true) },
                        relayPort = relayPort.takeIf { connectInstructions.routeKind.contains("relay", ignoreCase = true) },
                    )
                    controller.markRegisteringRelay()
                    controller.armRelayRegistrationBurst("connect_retry")
                    relayRegistered = awaitReceiverRegistered(
                        controlPlaneClient = controlPlaneClient,
                        baseUrl = baseUrl,
                        sessionId = session.sessionId,
                        sessionToken = session.sessionToken,
                    ).receiverRegistered
                }
                appendEventLog("RELAY REGISTER STATUS | registered=$relayRegistered | route=${connectInstructions.routeKind}")
                if (!relayRegistered) {
                    statusText = "Registering phone"
                }
                if (!relayRegistered && connectInstructions.routeKind.contains("relay", ignoreCase = true)) {
                    throw IllegalStateException("Receiver was not registered in relay.")
                }

                controlPlaneClient.activateSession(
                    baseUrl = baseUrl,
                    sessionId = session.sessionId,
                    sessionToken = session.sessionToken,
                )

                val hostReadySnapshot = awaitHostReady(
                    controlPlaneClient = controlPlaneClient,
                    baseUrl = baseUrl,
                    sessionId = session.sessionId,
                    sessionToken = session.sessionToken,
                )
                hostReady = hostReadySnapshot.hostReady
                activeRouteKind = hostReadySnapshot.routeKind

                val readyInstructions = controlPlaneClient.getConnectInstructions(
                    baseUrl = baseUrl,
                    sessionId = session.sessionId,
                    sessionToken = session.sessionToken,
                )
                hostReady = readyInstructions.hostReady

                screenState = MinimalReceiverScreenState.WaitingForStream
                statusText = "Waiting for stream"
                controller.markWaitingForFirstPacket()
                controller.armRelayRegistrationBurst("waiting_stream")
                controller.sendControlPing("connect")
                controller.requestKeyFrame("connect")
                delay(180L)
                controller.requestKeyFrame("connect_fast_retry_1")
                delay(220L)
                controller.requestKeyFrame("connect_fast_retry_2")

                if (!controller.awaitFirstPacket(12_000L)) {
                    throw IllegalStateException("Host did not start streaming.")
                }

                if (!isAttemptCurrent(attemptId)) {
                    throw IllegalStateException("Connection superseded.")
                }

                screenState = MinimalReceiverScreenState.Streaming
                statusText = "Connected"
                errorText = null
            } catch (t: Throwable) {
                dumpConnectionFailure(
                    attemptId = attemptId,
                    phase = "connect",
                    throwable = t,
                    sessionId = createdSessionId,
                    sessionToken = createdSessionToken,
                    baseUrl = baseUrl,
                )
                appendEventLog("CONNECTION FAILURE | attempt=$attemptId | error=${t.message ?: "unknown"} | createdSessionId=${createdSessionId.ifBlank { "-" }} | activeSessionId=${activeSessionId.ifBlank { "-" }} | lastStoppedSessionId=${lastStoppedSessionId.ifBlank { "-" }}")
                if (createdSessionId.isNotBlank() && createdSessionToken.isNotBlank()) {
                    runCatching<Unit> {
                        controlPlaneClient.stopSession(
                            baseUrl = baseUrl,
                            sessionId = createdSessionId,
                            sessionToken = createdSessionToken,
                            reason = "android_connect_failed",
                        )
                    }
                }
                if (isAttemptCurrent(attemptId)) {
                    controller.configureManagedRelayRoute(null, null, null, null)
                    controller.stop()
                    runCatching { controlPlaneClient.clearAllManagedSessionState() }
                    resetActiveState()
                    screenState = MinimalReceiverScreenState.Error
                    statusText = "Connection failed"
                    errorText = t.message ?: "Unknown connection error."
                }
            }
        }
    }

    val latestSessionId by rememberUpdatedState(activeSessionId)
    val latestSessionToken by rememberUpdatedState(activeSessionToken)
    val latestBaseUrl by rememberUpdatedState(controlPlaneUrl.trim())

    DisposableEffect(controller) {
        onDispose {
            if (latestSessionId.isNotBlank() && latestSessionToken.isNotBlank() && latestBaseUrl.isNotBlank()) {
                scope.launch {
                    runCatching<Unit> {
                        controlPlaneClient.stopSession(
                            baseUrl = latestBaseUrl,
                            sessionId = latestSessionId,
                            sessionToken = latestSessionToken,
                            reason = "android_app_close",
                        )
                    }
                    runCatching { controlPlaneClient.clearAllManagedSessionState() }
                }
            }
            controller.stop()
            onFullscreenChanged(false)
        }
    }

    LaunchedEffect(activeSessionId, activeSessionToken, controlPlaneUrl) {
        if (activeSessionId.isBlank() || activeSessionToken.isBlank()) {
            return@LaunchedEffect
        }

        while (activeSessionId.isNotBlank() && activeSessionToken.isNotBlank()) {
            delay(5_000L)
            runCatching {
                controlPlaneClient.keepAliveSession(
                    baseUrl = controlPlaneUrl.trim(),
                    sessionId = activeSessionId,
                    sessionToken = activeSessionToken,
                )
            }
        }
    }

    LaunchedEffect(uiState.phase, activeSessionId) {
        if (activeSessionId.isNotBlank() && uiState.phase == PcReceiverPhase.CONNECTED) {
            screenState = MinimalReceiverScreenState.Streaming
            statusText = "Connected"
            errorText = null
        }
    }

    LaunchedEffect(viewerVisible) {
        onFullscreenChanged(viewerVisible)
    }

    ReceiverImmersiveMode(enabled = viewerVisible)

    Surface(
        modifier = Modifier.fillMaxSize(),
        color = Color(0xFFF4F0E8),
    ) {
        Box(modifier = Modifier.fillMaxSize()) {
            if (viewerVisible) {
                MinimalReceiverViewer(
                    uiState = uiState,
                    controller = controller,
                    diagnostics = diagnostics,
                    hostLabel = activeHostLabel,
                    statusText = statusText,
                    onStop = { stopSession("android_receiver_stop") },
                    onOpenDiagnostics = { diagnosticsVisible = true },
                    onOpenLogs = { logsVisible = true },
                    previewAspectRatio = previewAspectRatio,
                )
            } else {
                MinimalReceiverConnectScreen(
                    pcCode = pcCode,
                    onPcCodeChange = { pcCode = it.filter(Char::isLetterOrDigit).take(4).uppercase() },
                    selectedPreset = selectedPreset,
                    availablePresets = PcDesiredStreamPreset.entries.filter { preset ->
                        preset != PcDesiredStreamPreset.AV1_MAX ||
                            settingsVisible ||
                            supportedDecodeCodecs.any { it.equals("video/av1", ignoreCase = true) }
                    },
                    onPresetSelected = { selectedPresetName = it.name },
                    statusText = statusText,
                    errorText = errorText,
                    screenState = screenState,
                    onConnect = ::connect,
                    onOpenSettings = { settingsVisible = true },
                    onOpenDiagnostics = { diagnosticsVisible = true },
                    onOpenLogs = { logsVisible = true },
                    canConnect = screenState != MinimalReceiverScreenState.Connecting &&
                        screenState != MinimalReceiverScreenState.RegisteringRelay &&
                        screenState != MinimalReceiverScreenState.WaitingForStream &&
                        screenState != MinimalReceiverScreenState.Stopping,
                )
            }

            if (settingsVisible) {
                ReceiverSettingsSheet(
                    controlPlaneUrl = controlPlaneUrl,
                    onControlPlaneUrlChange = { controlPlaneUrl = it },
                    listenPortText = listenPortText,
                    onListenPortChange = { listenPortText = it.filter(Char::isDigit).take(5) },
                    clientRegionText = clientRegionText,
                    onClientRegionChange = { clientRegionText = it },
                    preferHevc = preferHevc,
                    onPreferHevcChange = { preferHevc = it },
                    preferNativeDecoder = preferNativeDecoder,
                    onPreferNativeDecoderChange = { preferNativeDecoder = it },
                    preferRelay = preferRelay,
                    onPreferRelayChange = { preferRelay = it },
                    requestAudio = requestAudio,
                    onRequestAudioChange = { requestAudio = it },
                    requestedWidthText = requestedWidthText,
                    onRequestedWidthChange = { requestedWidthText = it.filter(Char::isDigit).take(4) },
                    requestedHeightText = requestedHeightText,
                    onRequestedHeightChange = { requestedHeightText = it.filter(Char::isDigit).take(4) },
                    requestedFpsText = requestedFpsText,
                    onRequestedFpsChange = { requestedFpsText = it.filter(Char::isDigit).take(3) },
                    requestedBitrateText = requestedBitrateText,
                    onRequestedBitrateChange = { requestedBitrateText = it.filter { ch -> ch.isDigit() || ch == '.' || ch == ',' }.take(6) },
                    selectedPreset = selectedPreset,
                    onDismiss = { settingsVisible = false },
                )
            }

            if (diagnosticsVisible) {
                ReceiverDiagnosticsSheet(
                    diagnostics = diagnostics,
                    routeKind = activeRouteKind,
                    relayEndpoint = diagnostics.relayEndpoint,
                    packetsReceived = uiState.metrics.packetsReceived,
                    codecLabel = uiState.metrics.codecLabel,
                    resolutionLabel = uiState.metrics.resolutionLabel,
                    decodeFps = uiState.metrics.decodeFps,
                    decoderPath = uiState.metrics.decoderPath,
                    decoderTuning = uiState.metrics.decoderTuning,
                    lastGamepadKey = uiState.metrics.lastGamepadKey,
                    lastGamepadMotion = uiState.metrics.lastGamepadMotion,
                    hostReady = hostReady,
                    onDismiss = { diagnosticsVisible = false },
                )
            }

            if (logsVisible) {
                ReceiverLogsSheet(
                    logs = eventLogs.toList(),
                    onDismiss = { logsVisible = false },
                )
            }
        }
    }
}

@Composable
private fun MinimalReceiverConnectScreen(
    pcCode: String,
    onPcCodeChange: (String) -> Unit,
    selectedPreset: PcDesiredStreamPreset,
    availablePresets: List<PcDesiredStreamPreset>,
    onPresetSelected: (PcDesiredStreamPreset) -> Unit,
    statusText: String,
    errorText: String?,
    screenState: MinimalReceiverScreenState,
    onConnect: () -> Unit,
    onOpenSettings: () -> Unit,
    onOpenDiagnostics: () -> Unit,
    onOpenLogs: () -> Unit,
    canConnect: Boolean,
) {
    val heroStatus = when (screenState) {
        MinimalReceiverScreenState.Idle -> "Ready to connect"
        MinimalReceiverScreenState.Connecting -> "Connecting"
        MinimalReceiverScreenState.RegisteringRelay -> "Registering relay"
        MinimalReceiverScreenState.WaitingForStream -> "Waiting for stream"
        MinimalReceiverScreenState.Streaming -> "Streaming"
        MinimalReceiverScreenState.Stopping -> "Stopping"
        MinimalReceiverScreenState.Error -> "Connection issue"
    }

    Column(
        modifier = Modifier
            .fillMaxSize()
            .statusBarsPadding()
            .verticalScroll(rememberScrollState())
            .padding(horizontal = 20.dp, vertical = 16.dp),
        verticalArrangement = Arrangement.spacedBy(18.dp),
    ) {
        ElevatedCard(
            colors = CardDefaults.elevatedCardColors(
                containerColor = Color(0xFF14120F),
            ),
            elevation = CardDefaults.elevatedCardElevation(defaultElevation = 6.dp),
        ) {
            Row(
                modifier = Modifier.padding(18.dp),
                horizontalArrangement = Arrangement.spacedBy(16.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Column(
                    modifier = Modifier.weight(1f),
                    verticalArrangement = Arrangement.spacedBy(6.dp),
                ) {
                    Text(
                        text = "Everty Remote",
                        style = MaterialTheme.typography.headlineMedium,
                        fontWeight = FontWeight.Bold,
                        color = Color.White,
                    )
                    Text(
                        text = heroStatus,
                        style = MaterialTheme.typography.titleMedium,
                        fontWeight = FontWeight.SemiBold,
                        color = Color(0xFF8BD3FF),
                    )
                    Text(
                        text = "Connect to your PC with a short code. Preset stays saved on reconnect.",
                        style = MaterialTheme.typography.bodyMedium,
                        color = Color.White.copy(alpha = 0.74f),
                    )
                }
                OutlinedButton(onClick = onOpenSettings) {
                    Text("Advanced")
                }
            }
        }

        Column(verticalArrangement = Arrangement.spacedBy(18.dp)) {
            Text(
                text = "Quick presets",
                style = MaterialTheme.typography.titleMedium,
                fontWeight = FontWeight.SemiBold,
                color = Color(0xFF14120F),
            )

            ElevatedCard(
                colors = CardDefaults.elevatedCardColors(
                    containerColor = Color(0xFFFFFBF4),
                ),
                elevation = CardDefaults.elevatedCardElevation(defaultElevation = 4.dp),
            ) {
                Column(
                    modifier = Modifier.padding(20.dp),
                    verticalArrangement = Arrangement.spacedBy(16.dp),
                ) {
                    Text(
                        text = "PC code",
                        style = MaterialTheme.typography.titleMedium,
                        fontWeight = FontWeight.SemiBold,
                        color = Color(0xFF14120F),
                    )

                    OutlinedTextField(
                        value = pcCode,
                        onValueChange = onPcCodeChange,
                        modifier = Modifier.fillMaxWidth(),
                        singleLine = true,
                        label = { Text("Enter 4-character code") },
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Ascii),
                    )

                    Button(
                        modifier = Modifier.fillMaxWidth(),
                        enabled = canConnect && pcCode.length == 4,
                        onClick = onConnect,
                ) {
                    Text(if (screenState == MinimalReceiverScreenState.Error) "Try again" else "Connect")
                    }

                    Text(
                        text = "Preset packs",
                        style = MaterialTheme.typography.titleSmall,
                        fontWeight = FontWeight.SemiBold,
                        color = Color(0xFF14120F),
                    )

                    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                        availablePresets.chunked(2).forEach { rowPresets ->
                            Row(
                                modifier = Modifier.fillMaxWidth(),
                                horizontalArrangement = Arrangement.spacedBy(10.dp),
                            ) {
                                rowPresets.forEach { preset ->
                                    ElevatedCard(
                                        modifier = Modifier
                                            .weight(1f)
                                            .height(92.dp)
                                            .clickable { onPresetSelected(preset) },
                                        elevation = CardDefaults.elevatedCardElevation(
                                            defaultElevation = if (preset == selectedPreset) 8.dp else 2.dp,
                                        ),
                                        colors = CardDefaults.elevatedCardColors(
                                            containerColor = if (preset == selectedPreset) Color(0xFF1B1A18) else Color.White,
                                        ),
                                    ) {
                                        Column(
                                            modifier = Modifier.padding(14.dp),
                                            verticalArrangement = Arrangement.spacedBy(6.dp),
                                        ) {
                                            Text(
                                                text = preset.title,
                                                style = MaterialTheme.typography.titleSmall,
                                                fontWeight = FontWeight.SemiBold,
                                                color = if (preset == selectedPreset) Color.White else Color(0xFF14120F),
                                            )
                                            Text(
                                                text = preset.summary,
                                                style = MaterialTheme.typography.bodySmall,
                                                color = if (preset == selectedPreset) Color.White.copy(alpha = 0.82f) else Color(0xFF5C544A),
                                            )
                                            if (preset == selectedPreset) {
                                                Text(
                                                    text = "Selected",
                                                    style = MaterialTheme.typography.labelSmall,
                                                    fontWeight = FontWeight.Bold,
                                                    color = Color(0xFF8BD3FF),
                                                )
                                            }
                                        }
                                    }
                                }
                                if (rowPresets.size == 1) {
                                    Spacer(modifier = Modifier.weight(1f).height(92.dp))
                                }
                            }
                        }
                    }

                    Text(
                        text = if (screenState == MinimalReceiverScreenState.Idle) "Pick one preset, then connect." else statusText,
                        style = MaterialTheme.typography.bodyMedium,
                        color = Color(0xFF5C544A),
                    )

                    errorText?.let {
                        Text(
                            text = it,
                            style = MaterialTheme.typography.bodyMedium,
                            color = MaterialTheme.colorScheme.error,
                        )
                    }
                }
            }
        }

        Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            OutlinedButton(
                modifier = Modifier.weight(1f),
                onClick = onOpenSettings,
            ) {
                Text("Advanced")
            }
            OutlinedButton(
                modifier = Modifier.weight(1f),
                onClick = onOpenDiagnostics,
            ) {
                Text("Diagnostics")
            }
            OutlinedButton(
                modifier = Modifier.weight(1f),
                onClick = onOpenLogs,
            ) {
                Text("Logs")
            }
        }
    }
}

@Composable
private fun MinimalReceiverViewer(
    uiState: PcReceiverUiState,
    controller: PcReceiverClientController,
    diagnostics: PcReceiverDiagnostics,
    hostLabel: String,
    statusText: String,
    onStop: () -> Unit,
    onOpenDiagnostics: () -> Unit,
    onOpenLogs: () -> Unit,
    previewAspectRatio: Float,
) {
    val transformState = remember { PreviewTransformState() }
    val focusRequester = remember { FocusRequester() }
    val context = LocalContext.current
    val preferSurfaceView = remember(context) {
        isAndroidTvDevice(context) || Build.VERSION.SDK_INT <= Build.VERSION_CODES.N_MR1
    }
    var chromeVisible by rememberSaveable { mutableStateOf(true) }
    val connected = uiState.phase == PcReceiverPhase.CONNECTED

    LaunchedEffect(connected) {
        if (connected) {
            delay(2_000L)
            chromeVisible = false
        } else {
            chromeVisible = true
        }
    }

    LaunchedEffect(Unit) {
        focusRequester.requestFocus()
    }

    Box(
        modifier = Modifier
            .fillMaxSize()
            .background(Color.Black)
            .focusRequester(focusRequester)
            .focusable()
            .onPreviewKeyEvent { keyEvent ->
                val nativeEvent = keyEvent.nativeKeyEvent
                if (nativeEvent != null && nativeEvent.action == KeyEvent.ACTION_DOWN) {
                    when (nativeEvent.keyCode) {
                        KeyEvent.KEYCODE_BACK,
                        KeyEvent.KEYCODE_ESCAPE,
                        KeyEvent.KEYCODE_MENU,
                        KeyEvent.KEYCODE_SETTINGS -> {
                            chromeVisible = !chromeVisible
                            true
                        }
                        else -> false
                    }
                } else if (nativeEvent != null && controller.handleKeyEvent(nativeEvent)) {
                    true
                } else {
                    false
                }
            },
    ) {
        Box(
            modifier = Modifier
                .fillMaxSize(),
            contentAlignment = Alignment.Center,
        ) {
            ReceiverPreview(
                controller = controller,
                useLegacySurfaceView = preferSurfaceView,
                videoWidth = uiState.metrics.videoWidth,
                videoHeight = uiState.metrics.videoHeight,
                previewAspectRatio = previewAspectRatio,
                transformState = transformState,
                onLocalPointer = { _, _ -> },
                onToggleChrome = { chromeVisible = !chromeVisible },
                onResetZoom = {
                    transformState.reset()
                    chromeVisible = true
                },
            )
        }

        if (uiState.phase != PcReceiverPhase.CONNECTED) {
            Box(
                modifier = Modifier
                    .fillMaxSize()
                    .background(Color.Black.copy(alpha = 0.42f)),
                contentAlignment = Alignment.Center,
            ) {
                Text(
                    text = statusText,
                    color = Color.White,
                    style = MaterialTheme.typography.bodyLarge,
                )
            }
        }

        Surface(
            modifier = Modifier
                .align(Alignment.TopEnd)
                .statusBarsPadding()
                .padding(14.dp),
            shape = RoundedCornerShape(16.dp),
            color = Color(0xFF1B1A18).copy(alpha = 0.82f),
        ) {
            OutlinedButton(
                modifier = Modifier.focusProperties { canFocus = false },
                onClick = { chromeVisible = !chromeVisible },
            ) {
                Text(if (chromeVisible) "Hide" else "Menu")
            }
        }

        if (chromeVisible) {
            Surface(
                modifier = Modifier
                    .align(Alignment.TopCenter)
                    .statusBarsPadding()
                    .padding(14.dp),
                shape = RoundedCornerShape(18.dp),
                color = Color(0xFF1B1A18).copy(alpha = 0.82f),
            ) {
                Row(
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(horizontal = 14.dp, vertical = 12.dp),
                    horizontalArrangement = Arrangement.spacedBy(12.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Column(
                        modifier = Modifier.weight(1f),
                        verticalArrangement = Arrangement.spacedBy(2.dp),
                    ) {
                        Text(
                            text = hostLabel,
                            color = Color.White,
                            style = MaterialTheme.typography.bodyLarge,
                            fontWeight = FontWeight.SemiBold,
                        )
                        Text(
                            text = buildString {
                                append(if (diagnostics.receiverRegistered) "Streaming" else statusText)
                                if (transformState.isZoomed) {
                                    append(" · ")
                                    append(String.format("%.1fx", transformState.scale))
                                }
                            },
                            color = Color.White.copy(alpha = 0.78f),
                            style = MaterialTheme.typography.bodySmall,
                        )
                    }
                    OutlinedButton(
                        modifier = Modifier.focusProperties { canFocus = false },
                        onClick = onOpenDiagnostics,
                    ) {
                        Text("Info")
                    }
                    OutlinedButton(
                        modifier = Modifier.focusProperties { canFocus = false },
                        onClick = onOpenLogs,
                    ) {
                        Text("Logs")
                    }
                    Button(
                        modifier = Modifier.focusProperties { canFocus = false },
                        onClick = onStop,
                    ) {
                        Text("Stop")
                    }
                }
            }

            Surface(
                modifier = Modifier
                    .align(Alignment.BottomStart)
                    .padding(14.dp),
                shape = RoundedCornerShape(16.dp),
                color = Color(0xFF1B1A18).copy(alpha = 0.72f),
            ) {
                Row(
                    modifier = Modifier.padding(horizontal = 12.dp, vertical = 10.dp),
                    horizontalArrangement = Arrangement.spacedBy(16.dp),
                ) {
                    CompactMetric("Codec", uiState.metrics.codecLabel)
                    CompactMetric("Resolution", uiState.metrics.resolutionLabel)
                    CompactMetric("FPS", uiState.metrics.decodeFps.toString())
                    CompactMetric("Packets", uiState.metrics.packetsReceived.toString())
                }
            }
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun ReceiverSettingsSheet(
    controlPlaneUrl: String,
    onControlPlaneUrlChange: (String) -> Unit,
    listenPortText: String,
    onListenPortChange: (String) -> Unit,
    clientRegionText: String,
    onClientRegionChange: (String) -> Unit,
    preferHevc: Boolean,
    onPreferHevcChange: (Boolean) -> Unit,
    preferNativeDecoder: Boolean,
    onPreferNativeDecoderChange: (Boolean) -> Unit,
    preferRelay: Boolean,
    onPreferRelayChange: (Boolean) -> Unit,
    requestAudio: Boolean,
    onRequestAudioChange: (Boolean) -> Unit,
    requestedWidthText: String,
    onRequestedWidthChange: (String) -> Unit,
    requestedHeightText: String,
    onRequestedHeightChange: (String) -> Unit,
    requestedFpsText: String,
    onRequestedFpsChange: (String) -> Unit,
    requestedBitrateText: String,
    onRequestedBitrateChange: (String) -> Unit,
    selectedPreset: PcDesiredStreamPreset,
    onDismiss: () -> Unit,
) {
    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 20.dp, vertical = 8.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text(
                text = "Settings",
                style = MaterialTheme.typography.titleMedium,
                fontWeight = FontWeight.SemiBold,
            )
            Text(
                text = "Preset: ${selectedPreset.title} (${selectedPreset.summary})",
                style = MaterialTheme.typography.bodyMedium,
                color = Color(0xFF5C544A),
            )
            OutlinedTextField(
                value = controlPlaneUrl,
                onValueChange = onControlPlaneUrlChange,
                modifier = Modifier.fillMaxWidth(),
                label = { Text("Control plane URL") },
                singleLine = true,
            )
            Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                OutlinedTextField(
                    value = listenPortText,
                    onValueChange = onListenPortChange,
                    modifier = Modifier.weight(1f),
                    label = { Text("UDP port") },
                    singleLine = true,
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
                )
                OutlinedTextField(
                    value = clientRegionText,
                    onValueChange = onClientRegionChange,
                    modifier = Modifier.weight(1f),
                    label = { Text("Region") },
                    singleLine = true,
                )
            }
            Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                OutlinedTextField(
                    value = requestedWidthText,
                    onValueChange = onRequestedWidthChange,
                    modifier = Modifier.weight(1f),
                    label = { Text("Width") },
                    singleLine = true,
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
                )
                OutlinedTextField(
                    value = requestedHeightText,
                    onValueChange = onRequestedHeightChange,
                    modifier = Modifier.weight(1f),
                    label = { Text("Height") },
                    singleLine = true,
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
                )
            }
            Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                OutlinedTextField(
                    value = requestedFpsText,
                    onValueChange = onRequestedFpsChange,
                    modifier = Modifier.weight(1f),
                    label = { Text("FPS") },
                    singleLine = true,
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
                )
                OutlinedTextField(
                    value = requestedBitrateText,
                    onValueChange = onRequestedBitrateChange,
                    modifier = Modifier.weight(1f),
                    label = { Text("Mbps") },
                    singleLine = true,
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Decimal),
                )
            }
            CompactToggle("Prefer HEVC", preferHevc, onPreferHevcChange)
            CompactToggle("Prefer native decoder", preferNativeDecoder, onPreferNativeDecoderChange)
            CompactToggle("Prefer relay", preferRelay, onPreferRelayChange)
            CompactToggle("Request audio", requestAudio, onRequestAudioChange)
            Spacer(modifier = Modifier.height(12.dp))
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun ReceiverDiagnosticsSheet(
    diagnostics: PcReceiverDiagnostics,
    routeKind: String,
    relayEndpoint: String,
    packetsReceived: Long,
    codecLabel: String,
    resolutionLabel: String,
    decodeFps: Int,
    decoderPath: String,
    decoderTuning: String,
    lastGamepadKey: String,
    lastGamepadMotion: String,
    hostReady: Boolean,
    onDismiss: () -> Unit,
) {
    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 20.dp, vertical = 8.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Text(
                text = "Diagnostics",
                style = MaterialTheme.typography.titleMedium,
                fontWeight = FontWeight.SemiBold,
            )
            MetricLine("Session ID", diagnostics.sessionId)
            MetricLine("Route", routeKind.ifBlank { "-" })
            MetricLine("Relay", relayEndpoint.ifBlank { "-" })
            MetricLine("Receiver registered", if (diagnostics.receiverRegistered) "yes" else "no")
            MetricLine("Host ready", if (hostReady) "yes" else "no")
            MetricLine("Last relay ack", diagnostics.lastRelayAck)
            MetricLine("First packet", if (diagnostics.firstPacketReceived) "yes" else "no")
            MetricLine("Packets", packetsReceived.toString())
            Text(
                text = "Decoder Performance",
                style = MaterialTheme.typography.titleSmall,
                fontWeight = FontWeight.SemiBold,
            )
            MetricLine("Codec", codecLabel)
            MetricLine("Resolution", resolutionLabel)
            MetricLine("Decode FPS", decodeFps.toString())
            MetricLine("Decoder path", decoderPath)
            MetricLine("Decoder tuning", decoderTuning)
            MetricLine("Last gamepad key", lastGamepadKey)
            MetricLine("Last gamepad motion", lastGamepadMotion)
            MetricLine("Last control error", diagnostics.lastControlSendError)
            MetricLine("Last media packet", diagnostics.lastMediaPacketFrom)
            Spacer(modifier = Modifier.height(12.dp))
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun ReceiverLogsSheet(
    logs: List<String>,
    onDismiss: () -> Unit,
) {
    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 20.dp, vertical = 8.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Text(
                text = "Logs",
                style = MaterialTheme.typography.titleMedium,
                fontWeight = FontWeight.SemiBold,
            )
            SelectionContainer {
                Text(
                    text = if (logs.isEmpty()) "No logs yet." else logs.joinToString("\n"),
                    style = MaterialTheme.typography.bodySmall,
                )
            }
            Spacer(modifier = Modifier.height(12.dp))
        }
    }
}

@Composable
private fun CompactToggle(
    title: String,
    checked: Boolean,
    onCheckedChange: (Boolean) -> Unit,
) {
    Row(
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            text = title,
            style = MaterialTheme.typography.bodyMedium,
        )
        Switch(
            checked = checked,
            onCheckedChange = onCheckedChange,
        )
    }
}

@Composable
private fun CompactMetric(label: String, value: String) {
    Column(
        horizontalAlignment = Alignment.Start,
        verticalArrangement = Arrangement.spacedBy(2.dp),
    ) {
        Text(
            text = label,
            style = MaterialTheme.typography.bodySmall,
            color = Color.White.copy(alpha = 0.68f),
        )
        Text(
            text = value,
            style = MaterialTheme.typography.bodyMedium,
            fontWeight = FontWeight.Medium,
            color = Color.White,
        )
    }
}

private fun parseOptionalInt(value: String): Int? =
    value.trim().takeIf { it.isNotEmpty() }?.toIntOrNull()

private fun parseBitrateMbpsToBps(value: String): Int? {
    val normalized = value.trim().replace(',', '.')
    if (normalized.isEmpty()) {
        return null
    }
    val mbps = normalized.toDoubleOrNull() ?: return null
    return if (mbps > 0.0) (mbps * 1_000_000.0).toInt() else null
}

@Composable
private fun ReceiverPreview(
    controller: PcReceiverClientController,
    useLegacySurfaceView: Boolean,
    videoWidth: Int,
    videoHeight: Int,
    previewAspectRatio: Float,
    transformState: PreviewTransformState,
    onLocalPointer: (Float, Float) -> Unit,
    onToggleChrome: () -> Unit,
    onResetZoom: () -> Unit,
) {
    AndroidView(
        modifier = Modifier
            .fillMaxWidth()
            .aspectRatio(previewAspectRatio),
        factory = { androidContext ->
            if (useLegacySurfaceView) {
                SurfaceView(androidContext).apply {
                    setZOrderOnTop(false)
                    setZOrderMediaOverlay(false)
                    holder.addCallback(
                        object : SurfaceHolder.Callback {
                            override fun surfaceCreated(holder: SurfaceHolder) {
                                controller.attachSurface(holder.surface)
                            }

                            override fun surfaceChanged(holder: SurfaceHolder, format: Int, width: Int, height: Int) {
                                controller.attachSurface(holder.surface)
                            }

                            override fun surfaceDestroyed(holder: SurfaceHolder) {
                                controller.attachSurface(null)
                            }
                        },
                    )
                    configureRemoteInput(
                        controller = controller,
                        onLocalPointer = onLocalPointer,
                        transformState = transformState,
                        allowLocalTransform = false,
                        onToggleChrome = onToggleChrome,
                        onResetZoom = onResetZoom,
                    )
                }
            } else {
                TextureView(androidContext).apply {
                    isOpaque = true
                    surfaceTextureListener =
                        object : TextureView.SurfaceTextureListener {
                            private var attachedSurface: Surface? = null

                            override fun onSurfaceTextureAvailable(surfaceTexture: SurfaceTexture, width: Int, height: Int) {
                                attachedSurface = Surface(surfaceTexture)
                                controller.attachSurface(attachedSurface)
                            }

                            override fun onSurfaceTextureSizeChanged(surfaceTexture: SurfaceTexture, width: Int, height: Int) {
                                attachedSurface?.let(controller::attachSurface)
                            }

                            override fun onSurfaceTextureDestroyed(surfaceTexture: SurfaceTexture): Boolean {
                                controller.attachSurface(null)
                                attachedSurface?.release()
                                attachedSurface = null
                                return true
                            }

                            override fun onSurfaceTextureUpdated(surfaceTexture: SurfaceTexture) = Unit
                        }
                    configureRemoteInput(
                        controller = controller,
                        onLocalPointer = onLocalPointer,
                        transformState = transformState,
                        allowLocalTransform = true,
                        onToggleChrome = onToggleChrome,
                        onResetZoom = onResetZoom,
                    )
                }
            }
        },
        update = { view ->
            transformState.updateViewport(view.width, view.height)
            when (view) {
                is SurfaceView -> {
                    if (videoWidth > 0 && videoHeight > 0) {
                        view.holder.setFixedSize(videoWidth, videoHeight)
                    }
                }

                is TextureView -> {
                    if (view.isAvailable && videoWidth > 0 && videoHeight > 0) {
                        view.surfaceTexture?.setDefaultBufferSize(videoWidth, videoHeight)
                    }
                    view.pivotX = view.width / 2f
                    view.pivotY = view.height / 2f
                    view.scaleX = transformState.scale
                    view.scaleY = transformState.scale
                    view.translationX = transformState.translationX
                    view.translationY = transformState.translationY
                }
            }
        },
    )
}

private fun View.configureRemoteInput(
    controller: PcReceiverClientController,
    onLocalPointer: (Float, Float) -> Unit,
    transformState: PreviewTransformState,
    allowLocalTransform: Boolean,
    onToggleChrome: () -> Unit,
    onResetZoom: () -> Unit,
) {
    isFocusable = true
    isFocusableInTouchMode = true
    keepScreenOn = true
    requestFocus()
    post { requestFocus() }

    val scaleGestureDetector =
        if (allowLocalTransform) {
            ScaleGestureDetector(
                context,
                object : ScaleGestureDetector.SimpleOnScaleGestureListener() {
                    override fun onScale(detector: ScaleGestureDetector): Boolean {
                        transformState.applyScale(detector.scaleFactor, detector.focusX, detector.focusY)
                        scaleX = transformState.scale
                        scaleY = transformState.scale
                        translationX = transformState.translationX
                        translationY = transformState.translationY
                        return true
                    }
                },
            )
        } else {
            null
        }

    val gestureDetector =
        if (allowLocalTransform) {
            GestureDetector(
                context,
                object : GestureDetector.SimpleOnGestureListener() {
                    override fun onDown(e: MotionEvent): Boolean = true

                    override fun onDoubleTap(e: MotionEvent): Boolean {
                        onResetZoom()
                        scaleX = transformState.scale
                        scaleY = transformState.scale
                        translationX = transformState.translationX
                        translationY = transformState.translationY
                        return true
                    }

                    override fun onSingleTapConfirmed(e: MotionEvent): Boolean {
                        onToggleChrome()
                        return true
                    }

                    override fun onScroll(
                        e1: MotionEvent?,
                        e2: MotionEvent,
                        distanceX: Float,
                        distanceY: Float,
                    ): Boolean {
                        if (!transformState.isZoomed) {
                            return false
                        }
                        transformState.panBy(-distanceX, -distanceY)
                        scaleX = transformState.scale
                        scaleY = transformState.scale
                        translationX = transformState.translationX
                        translationY = transformState.translationY
                        return true
                    }
                },
            )
        } else {
            GestureDetector(
                context,
                object : GestureDetector.SimpleOnGestureListener() {
                    override fun onSingleTapConfirmed(e: MotionEvent): Boolean {
                        onToggleChrome()
                        return true
                    }
                },
            )
        }

    var localTransformActive = false
    var lastMultiTouchAtMs = 0L

    setOnTouchListener { view, event ->
        if (event.isFromSource(InputDevice.SOURCE_MOUSE) &&
            (event.buttonState and (MotionEvent.BUTTON_SECONDARY or MotionEvent.BUTTON_TERTIARY)) != 0
        ) {
            if (event.actionMasked == MotionEvent.ACTION_UP || event.actionMasked == MotionEvent.ACTION_BUTTON_RELEASE) {
                onToggleChrome()
            }
            return@setOnTouchListener true
        }

        if (allowLocalTransform) {
            transformState.updateViewport(view.width, view.height)
            if (event.pointerCount > 1) {
                localTransformActive = true
                lastMultiTouchAtMs = SystemClock.uptimeMillis()
            }
            scaleGestureDetector?.onTouchEvent(event)
            gestureDetector?.onTouchEvent(event)
            if (event.actionMasked == MotionEvent.ACTION_UP || event.actionMasked == MotionEvent.ACTION_CANCEL) {
                if (event.pointerCount <= 1 && SystemClock.uptimeMillis() - lastMultiTouchAtMs > 120L) {
                    localTransformActive = false
                }
            }
            if (transformState.isZoomed && event.pointerCount == 1) {
                localTransformActive = true
            }
        }
        if (event.actionMasked != MotionEvent.ACTION_CANCEL) {
            onLocalPointer(event.x, event.y)
        }
        if (localTransformActive) {
            if (event.actionMasked == MotionEvent.ACTION_UP || event.actionMasked == MotionEvent.ACTION_CANCEL) {
                localTransformActive = false
            }
            true
        } else {
            controller.handlePointerEvent(event, view.width, view.height)
        }
    }
    setOnGenericMotionListener { view, event ->
        onLocalPointer(event.x, event.y)
        controller.handlePointerEvent(event, view.width, view.height)
    }
    setOnKeyListener { _, _, event ->
        controller.handleKeyEvent(event)
    }
}

@Composable
private fun MetricLine(label: String, value: String) {
    Row(
        modifier = Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.SpaceBetween,
    ) {
        Text(
            text = label,
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Text(
            text = value,
            style = MaterialTheme.typography.bodyMedium,
            fontWeight = FontWeight.Medium,
        )
    }
}
