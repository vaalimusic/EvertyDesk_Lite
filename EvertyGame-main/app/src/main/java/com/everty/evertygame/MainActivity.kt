package com.everty.evertygame

import android.Manifest
import android.app.Activity
import android.content.pm.PackageManager
import android.media.projection.MediaProjectionManager
import android.os.Build
import android.os.Bundle
import android.os.SystemClock
import android.view.KeyEvent
import android.view.MotionEvent
import androidx.activity.ComponentActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.ElevatedCard
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import com.everty.evertygame.receiver.PcReceiverClientController
import com.everty.evertygame.receiver.PcReceiverModeScreen
import com.everty.evertygame.input.GamepadBoostSupport
import com.everty.evertygame.stream.AdaptationMode
import com.everty.evertygame.stream.LatencyLabController
import com.everty.evertygame.stream.QualityPreset
import com.everty.evertygame.stream.StreamConfig
import com.everty.evertygame.stream.StreamPhase
import com.everty.evertygame.stream.StreamTransport
import com.everty.evertygame.stream.StreamUiState
import com.everty.evertygame.stream.StreamingService
import com.everty.evertygame.stream.StreamingSessionStore
import com.everty.evertygame.stream.VideoCodec
import com.everty.evertygame.touch.TouchLatencySprintController
import com.everty.evertygame.touch.TouchLatencySprintSupport
import com.everty.evertygame.ui.theme.EvertyGameTheme
import kotlin.math.roundToInt
import kotlinx.coroutines.delay

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()

        setContent {
            EvertyGameTheme {
                StreamControlApp(
                    projectionManager = getSystemService(MediaProjectionManager::class.java),
                )
            }
        }
    }

    override fun dispatchKeyEvent(event: KeyEvent): Boolean {
        val controller = PcReceiverClientController.activeController()
        if (controller != null && event.device != null && !event.isFromSource(android.view.InputDevice.SOURCE_MOUSE)) {
            controller.noteActivityKeyEvent(event)
        }
        if (controller != null && controller.handleKeyEvent(event)) {
            return true
        }
        return super.dispatchKeyEvent(event)
    }

    override fun dispatchGenericMotionEvent(event: MotionEvent): Boolean {
        val controller = PcReceiverClientController.activeController()
        if (controller != null && event.device != null) {
            controller.noteActivityMotionEvent(event)
        }
        if (controller != null && controller.handlePointerEvent(event, 1, 1)) {
            return true
        }
        return super.dispatchGenericMotionEvent(event)
    }
}

@Composable
private fun StreamControlApp(
    projectionManager: MediaProjectionManager,
) {
    var appModeName by rememberSaveable { mutableStateOf(AndroidAppMode.SENDER.name) }
    var pcReceiverFullscreen by rememberSaveable { mutableStateOf(false) }
    val appMode = AndroidAppMode.valueOf(appModeName)
    val showModeSwitcher = !(appMode == AndroidAppMode.PC_RECEIVER && pcReceiverFullscreen)

    Column(
        modifier = Modifier.fillMaxSize(),
    ) {
        if (showModeSwitcher) {
            AppModeSwitcher(
                selected = appMode,
                onSelected = { selected -> appModeName = selected.name },
            )
        }
        Box(
            modifier = Modifier
                .fillMaxWidth()
                .weight(1f),
        ) {
            when (appMode) {
                AndroidAppMode.SENDER -> SenderControlScreen(projectionManager)
                AndroidAppMode.PC_RECEIVER -> PcReceiverModeScreen(
                    onFullscreenChanged = { fullscreen -> pcReceiverFullscreen = fullscreen },
                )
            }
        }
    }
}

private enum class AndroidAppMode(val uiLabel: String) {
    SENDER("Android Sender"),
    PC_RECEIVER("PC Receiver"),
}

@Composable
private fun AppModeSwitcher(
    selected: AndroidAppMode,
    onSelected: (AndroidAppMode) -> Unit,
) {
    Surface(
        tonalElevation = 4.dp,
        color = MaterialTheme.colorScheme.surface,
    ) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .statusBarsPadding()
                .padding(horizontal = 20.dp, vertical = 12.dp),
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            AndroidAppMode.entries.forEach { mode ->
                val active = mode == selected
                Surface(
                    modifier = Modifier
                        .weight(1f)
                        .clickable { onSelected(mode) },
                    shape = RoundedCornerShape(18.dp),
                    color = if (active) {
                        MaterialTheme.colorScheme.primaryContainer
                    } else {
                        MaterialTheme.colorScheme.surfaceVariant
                    },
                ) {
                    Box(
                        modifier = Modifier.padding(horizontal = 16.dp, vertical = 14.dp),
                        contentAlignment = Alignment.Center,
                    ) {
                        Text(
                            text = mode.uiLabel,
                            style = MaterialTheme.typography.bodyLarge,
                            fontWeight = FontWeight.SemiBold,
                            color = if (active) {
                                MaterialTheme.colorScheme.onPrimaryContainer
                            } else {
                                MaterialTheme.colorScheme.onSurfaceVariant
                            },
                        )
                    }
                }
            }
        }
    }
}

@Composable
private fun SenderControlScreen(
    projectionManager: MediaProjectionManager,
) {
    val context = androidx.compose.ui.platform.LocalContext.current
    val uiState = StreamingSessionStore.uiState
    val supportedCodecs = remember { VideoCodec.supportedEncoders() }
    val audioSupported = Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q
    var host by rememberSaveable { mutableStateOf("192.168.0.5") }
    var portText by rememberSaveable { mutableStateOf("5001") }
    var transportName by rememberSaveable { mutableStateOf(StreamTransport.UDP.name) }
    var codecName by rememberSaveable { mutableStateOf(supportedCodecs.firstOrNull()?.name ?: VideoCodec.AVC.name) }
    var audioEnabled by rememberSaveable { mutableStateOf(false) }
    var presetName by rememberSaveable { mutableStateOf(QualityPreset.TOURNAMENT_FIGHTER.name) }
    var fpsText by rememberSaveable { mutableStateOf(QualityPreset.TOURNAMENT_FIGHTER.fps.toString()) }
    var bitrateMbpsText by rememberSaveable { mutableStateOf(formatBitrateMbps(QualityPreset.TOURNAMENT_FIGHTER.bitrateBps)) }
    var adaptationModeName by rememberSaveable { mutableStateOf(AdaptationMode.AUTO_BALANCED.name) }
    var touchLatencySprintEnabled by rememberSaveable { mutableStateOf(true) }
    var gamepadBoostEnabled by rememberSaveable { mutableStateOf(false) }
    var adaptiveRoiSplitStreamEnabled by rememberSaveable { mutableStateOf(true) }
    var inlineError by rememberSaveable { mutableStateOf<String?>(null) }
    var pendingConfig by remember { mutableStateOf<StreamConfig?>(null) }
    val selectedCodec = supportedCodecs.firstOrNull { it.name == codecName } ?: supportedCodecs.first()
    val selectedTransport = StreamTransport.valueOf(transportName)
    val touchLatencySprintServiceEnabled = TouchLatencySprintSupport.isServiceEnabled(context)
    val touchLatencySprintActive = TouchLatencySprintController.isSprintActive()
    var gamepadConnected by remember { mutableStateOf(GamepadBoostSupport.hasConnectedGamepad(context)) }

    DisposableEffect(context) {
        val monitor = GamepadBoostSupport.Monitor(
            context = context,
            handler = android.os.Handler(context.mainLooper),
        ) { connected ->
            gamepadConnected = connected
        }
        monitor.start()
        onDispose {
            monitor.close()
        }
    }

    val screenCaptureLauncher = rememberLauncherForActivityResult(
        contract = ActivityResultContracts.StartActivityForResult(),
    ) { result ->
        val config = pendingConfig
        pendingConfig = null

        if (result.resultCode == Activity.RESULT_OK && result.data != null && config != null) {
            inlineError = null
            StreamingService.start(
                context = context,
                resultCode = result.resultCode,
                projectionData = result.data!!,
                config = config,
            )
        } else {
            val message = "Разрешение на захват экрана не выдано"
            inlineError = message
            StreamingSessionStore.markError(message)
        }
    }

    val permissionsLauncher = rememberLauncherForActivityResult(
        contract = ActivityResultContracts.RequestMultiplePermissions(),
    ) { permissions ->
        val requestedConfig = pendingConfig ?: return@rememberLauncherForActivityResult
        val audioGranted = !requestedConfig.audioEnabled ||
            permissions[Manifest.permission.RECORD_AUDIO] != false

        val adjustedConfig = if (requestedConfig.audioEnabled && !audioGranted) {
            inlineError = "Audio permission denied. Starting video only."
            requestedConfig.copy(audioEnabled = false)
        } else {
            requestedConfig
        }

        pendingConfig = adjustedConfig
        screenCaptureLauncher.launch(projectionManager.createScreenCaptureIntent())
        StreamingSessionStore.markPermissionRequested(adjustedConfig)
    }

    fun beginCaptureRequest(config: StreamConfig) {
        pendingConfig = config

        val permissionsToRequest = buildList {
            if (
                Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
                ContextCompat.checkSelfPermission(
                    context,
                    Manifest.permission.POST_NOTIFICATIONS,
                ) != PackageManager.PERMISSION_GRANTED
            ) {
                add(Manifest.permission.POST_NOTIFICATIONS)
            }

            if (
                config.audioEnabled &&
                ContextCompat.checkSelfPermission(
                    context,
                    Manifest.permission.RECORD_AUDIO,
                ) != PackageManager.PERMISSION_GRANTED
            ) {
                add(Manifest.permission.RECORD_AUDIO)
            }
        }

        if (permissionsToRequest.isNotEmpty()) {
            permissionsLauncher.launch(permissionsToRequest.toTypedArray())
        } else {
            screenCaptureLauncher.launch(projectionManager.createScreenCaptureIntent())
            StreamingSessionStore.markPermissionRequested(config)
        }
    }

    fun startStreaming() {
        val config = buildConfigOrNull(
            host = host,
            portText = portText,
            transport = selectedTransport,
            preset = QualityPreset.valueOf(presetName),
            fpsText = fpsText,
            bitrateMbpsText = bitrateMbpsText,
            codec = selectedCodec,
            audioEnabled = audioEnabled && audioSupported,
            adaptationMode = AdaptationMode.valueOf(adaptationModeName),
            touchLatencySprintEnabled = touchLatencySprintEnabled,
            gamepadBoostEnabled = gamepadBoostEnabled,
            adaptiveRoiSplitStreamEnabled = adaptiveRoiSplitStreamEnabled,
        )

        if (config == null) {
            inlineError = "РЈРєР°Р¶Рё РєРѕСЂСЂРµРєС‚РЅС‹Рµ host Рё UDP-РїРѕСЂС‚"
        } else {
            beginCaptureRequest(config)
        }
    }

    Scaffold(
        modifier = Modifier.fillMaxSize(),
        containerColor = MaterialTheme.colorScheme.background,
        bottomBar = {
            StreamActionBar(
                canStart = !uiState.isBusy,
                canStop = uiState.phase == StreamPhase.STREAMING || uiState.phase == StreamPhase.STARTING,
                onStart = ::startStreaming,
                onStop = {
                    StreamingService.stop(context)
                    inlineError = null
                },
            )
        },
    ) { innerPadding ->
        Box(
            modifier = Modifier
                .fillMaxSize()
                .background(
                    brush = Brush.verticalGradient(
                        colors = listOf(
                            Color(0xFF08121D),
                            Color(0xFF0F2233),
                            MaterialTheme.colorScheme.background,
                        ),
                    ),
                )
                .padding(innerPadding),
        ) {
            Column(
                modifier = Modifier
                    .fillMaxSize()
                    .statusBarsPadding()
                    .verticalScroll(rememberScrollState())
                    .padding(horizontal = 20.dp, vertical = 18.dp),
                verticalArrangement = Arrangement.spacedBy(16.dp),
            ) {
                HeroBlock()
                SessionStatusCard(uiState = uiState)
                LatencyLabCard(uiState = uiState)
                TouchLatencySprintCard(
                    enabled = touchLatencySprintEnabled,
                    serviceEnabled = touchLatencySprintServiceEnabled,
                    sprintActive = touchLatencySprintActive,
                    onEnabledChange = { enabled ->
                        touchLatencySprintEnabled = enabled
                    },
                    onOpenAccessibilitySettings = {
                        TouchLatencySprintSupport.openAccessibilitySettings(context)
                    },
                )
                GamepadBoostCard(
                    enabled = gamepadBoostEnabled,
                    gamepadConnected = gamepadConnected,
                    onEnabledChange = { enabled ->
                        gamepadBoostEnabled = enabled
                    },
                )
                AdaptiveRoiSplitStreamCard(
                    enabled = adaptiveRoiSplitStreamEnabled,
                    onEnabledChange = { enabled ->
                        adaptiveRoiSplitStreamEnabled = enabled
                    },
                )

                ElevatedCard(
                    colors = CardDefaults.elevatedCardColors(
                        containerColor = MaterialTheme.colorScheme.surface,
                    ),
                ) {
                    Column(
                        modifier = Modifier.padding(18.dp),
                        verticalArrangement = Arrangement.spacedBy(12.dp),
                    ) {
                        Text(
                            text = "Подключение receiver",
                            style = MaterialTheme.typography.titleMedium,
                            fontWeight = FontWeight.SemiBold,
                        )
                        Text(
                            text = "Первый этап: Android sender отправляет H.264 по UDP в локальную сеть.",
                            style = MaterialTheme.typography.bodyMedium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                        TransportSelector(
                            selected = selectedTransport,
                            enabled = !uiState.isBusy,
                            onSelected = { transport ->
                                transportName = transport.name
                                if (transport == StreamTransport.ADB_TUNNEL_TCP) {
                                    host = "127.0.0.1"
                                    if (portText.isBlank()) {
                                        portText = "5001"
                                    }
                                } else if (host == "127.0.0.1") {
                                    host = "192.168.0.5"
                                }
                                inlineError = null
                            },
                        )
                        OutlinedTextField(
                            value = host,
                            onValueChange = {
                                host = it
                                inlineError = null
                            },
                            modifier = Modifier.fillMaxWidth(),
                            label = { Text("IP или hostname ПК") },
                            singleLine = true,
                            enabled = !uiState.isBusy,
                        )
                        OutlinedTextField(
                            value = portText,
                            onValueChange = {
                                portText = it.filter(Char::isDigit)
                                inlineError = null
                            },
                            modifier = Modifier.fillMaxWidth(),
                            label = { Text("UDP порт") },
                            singleLine = true,
                            enabled = !uiState.isBusy,
                            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
                        )
                    }
                }

                CodecSelector(
                    codecs = supportedCodecs,
                    selected = selectedCodec,
                    enabled = !uiState.isBusy,
                    onSelected = { codec ->
                        codecName = codec.name
                        inlineError = null
                    },
                )

                AudioSelectorCard(
                    enabled = !uiState.isBusy && audioSupported,
                    checked = audioEnabled && audioSupported,
                    supported = audioSupported,
                    onCheckedChange = { enabled ->
                        audioEnabled = enabled
                        inlineError = null
                    },
                )

                QualityPresetSelector(
                    selected = QualityPreset.valueOf(presetName),
                    enabled = !uiState.isBusy,
                    onSelected = { preset ->
                        presetName = preset.name
                        fpsText = preset.fps.toString()
                        bitrateMbpsText = formatBitrateMbps(preset.bitrateBps)
                        inlineError = null
                    },
                )

                CustomRateCard(
                    fpsText = fpsText,
                    bitrateMbpsText = bitrateMbpsText,
                    enabled = !uiState.isBusy,
                    onFpsChange = {
                        fpsText = it.filter(Char::isDigit).take(3)
                        inlineError = null
                    },
                    onBitrateMbpsChange = {
                        bitrateMbpsText = sanitizeBitrateInput(it)
                        inlineError = null
                    },
                )

                AdaptationModeSelector(
                    selected = AdaptationMode.valueOf(adaptationModeName),
                    enabled = !uiState.isBusy,
                    onSelected = { mode ->
                        adaptationModeName = mode.name
                        inlineError = null
                    },
                )

                inlineError?.let { message ->
                    ElevatedCard(
                        colors = CardDefaults.elevatedCardColors(
                            containerColor = MaterialTheme.colorScheme.errorContainer,
                        ),
                    ) {
                        Text(
                            text = message,
                            modifier = Modifier.padding(16.dp),
                            color = MaterialTheme.colorScheme.onErrorContainer,
                            style = MaterialTheme.typography.bodyMedium,
                        )
                    }
                }

                if (false) {
                    Row(
                    modifier = Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    Button(
                        modifier = Modifier.weight(1f),
                        enabled = !uiState.isBusy,
                        onClick = {
                            val config = buildConfigOrNull(
                                host = host,
                                portText = portText,
                                transport = selectedTransport,
                                preset = QualityPreset.valueOf(presetName),
                                fpsText = fpsText,
                                bitrateMbpsText = bitrateMbpsText,
                                codec = selectedCodec,
                                audioEnabled = audioEnabled && audioSupported,
                                adaptationMode = AdaptationMode.valueOf(adaptationModeName),
                                touchLatencySprintEnabled = touchLatencySprintEnabled,
                                gamepadBoostEnabled = gamepadBoostEnabled,
                                adaptiveRoiSplitStreamEnabled = adaptiveRoiSplitStreamEnabled,
                            )

                            if (config == null) {
                                inlineError = "Укажи корректные host и UDP-порт"
                            } else {
                                beginCaptureRequest(config)
                            }
                        },
                    ) {
                        Text("Start")
                    }
                    OutlinedButton(
                        modifier = Modifier.weight(1f),
                        enabled = uiState.phase == StreamPhase.STREAMING || uiState.phase == StreamPhase.STARTING,
                        onClick = {
                            StreamingService.stop(context)
                            inlineError = null
                        },
                    ) {
                        Text("Stop")
                    }
                }
                }

                Text(
                    text = "Протокол пакетов зафиксирован в docs/transport-protocol.md. Receiver можно реализовывать уже под него.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onBackground.copy(alpha = 0.75f),
                    modifier = Modifier.padding(bottom = 24.dp),
                )
            }
        }
    }
}

private fun buildConfigOrNull(
    host: String,
    portText: String,
    transport: StreamTransport,
    preset: QualityPreset,
    fpsText: String,
    bitrateMbpsText: String,
    codec: VideoCodec,
    audioEnabled: Boolean,
    adaptationMode: AdaptationMode,
    touchLatencySprintEnabled: Boolean,
    gamepadBoostEnabled: Boolean,
    adaptiveRoiSplitStreamEnabled: Boolean,
): StreamConfig? {
    val normalizedHost = host.trim()
    val port = portText.toIntOrNull()
    val fps = fpsText.toIntOrNull()
    val bitrateMbps = bitrateMbpsText.replace(',', '.').toFloatOrNull()
    val bitrateBps = bitrateMbps?.let { (it * 1_000_000f).toInt() }
    if (
        normalizedHost.isEmpty() ||
        port == null || port !in 1..65535 ||
        fps == null || fps !in 24..120 ||
        bitrateBps == null || bitrateBps !in 1_000_000..100_000_000
    ) {
        return null
    }
    return StreamConfig(
        host = normalizedHost,
        port = port,
        transport = transport,
        preset = preset,
        targetFps = fps,
        targetBitrateBps = bitrateBps,
        codec = codec,
        audioEnabled = audioEnabled,
        adaptationMode = adaptationMode,
        touchLatencySprintEnabled = touchLatencySprintEnabled,
        gamepadBoostEnabled = gamepadBoostEnabled,
        adaptiveRoiSplitStreamEnabled = adaptiveRoiSplitStreamEnabled,
    )
}

private fun formatBitrateMbps(bitrateBps: Int): String = "%.1f".format(bitrateBps / 1_000_000.0)

private fun sanitizeBitrateInput(value: String): String {
    val normalized = value.replace(',', '.')
    val builder = StringBuilder()
    var dotSeen = false
    normalized.forEach { ch ->
        when {
            ch.isDigit() -> builder.append(ch)
            ch == '.' && !dotSeen -> {
                builder.append(ch)
                dotSeen = true
            }
        }
    }
    return builder.toString()
}

@Composable
private fun StreamActionBar(
    canStart: Boolean,
    canStop: Boolean,
    onStart: () -> Unit,
    onStop: () -> Unit,
) {
    Surface(
        tonalElevation = 6.dp,
        shadowElevation = 8.dp,
        color = MaterialTheme.colorScheme.surface,
    ) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 20.dp, vertical = 14.dp),
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Button(
                modifier = Modifier.weight(1f),
                enabled = canStart,
                onClick = onStart,
            ) {
                Text("Start")
            }
            OutlinedButton(
                modifier = Modifier.weight(1f),
                enabled = canStop,
                onClick = onStop,
            ) {
                Text("Stop")
            }
        }
    }
}

@Composable
private fun CustomRateCard(
    fpsText: String,
    bitrateMbpsText: String,
    enabled: Boolean,
    onFpsChange: (String) -> Unit,
    onBitrateMbpsChange: (String) -> Unit,
) {
    ElevatedCard(
        colors = CardDefaults.elevatedCardColors(
            containerColor = MaterialTheme.colorScheme.surface,
        ),
    ) {
        Column(
            modifier = Modifier.padding(18.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text(
                text = "FPS и bitrate",
                style = MaterialTheme.typography.titleMedium,
                fontWeight = FontWeight.SemiBold,
            )
            Text(
                text = "Профиль теперь только задаёт базовое разрешение. FPS и bitrate можно крутить вручную.",
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                OutlinedTextField(
                    value = fpsText,
                    onValueChange = onFpsChange,
                    modifier = Modifier.weight(1f),
                    label = { Text("FPS") },
                    singleLine = true,
                    enabled = enabled,
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
                )
                OutlinedTextField(
                    value = bitrateMbpsText,
                    onValueChange = onBitrateMbpsChange,
                    modifier = Modifier.weight(1f),
                    label = { Text("Bitrate Mbps") },
                    singleLine = true,
                    enabled = enabled,
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Decimal),
                )
            }
            Text(
                text = "Диапазон: 24-120 FPS, 1-100 Mbps",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
}

@Composable
private fun HeroBlock() {
    ElevatedCard(
        colors = CardDefaults.elevatedCardColors(
            containerColor = Color(0xFF102637),
        ),
    ) {
        Column(
            modifier = Modifier.padding(20.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(
                text = "Everty Sender",
                style = MaterialTheme.typography.headlineMedium,
                color = Color(0xFFE8F3FF),
                fontWeight = FontWeight.Bold,
            )
            Text(
                text = "Захват экрана через MediaProjection, аппаратный H.264 и UDP-транспорт с приоритетом low latency.",
                style = MaterialTheme.typography.bodyLarge,
                color = Color(0xFFC6D9EA),
            )
        }
    }
}

@Composable
private fun LatencyLabCard(uiState: StreamUiState) {
    var autoPulse by rememberSaveable { mutableStateOf(false) }
    var pulseId by rememberSaveable { mutableStateOf(0) }
    var pulseSource by rememberSaveable { mutableStateOf("MANUAL") }
    var pulseIntervalMs by rememberSaveable { mutableStateOf(0) }
    var lastPulseRealtimeMs by rememberSaveable { mutableStateOf(0L) }
    var pulseAccentIndex by rememberSaveable { mutableStateOf(0) }
    var flashActive by remember { mutableStateOf(false) }
    var pendingTapNs by remember { mutableStateOf<Long?>(null) }
    var tapToUiMs by rememberSaveable { mutableStateOf(0) }
    var bestTapToUiMs by rememberSaveable { mutableStateOf<Int?>(null) }
    var worstTapToUiMs by rememberSaveable { mutableStateOf<Int?>(null) }

    val accentPalette = listOf(
        Color(0xFF1E4A8A),
        Color(0xFF0F7A52),
        Color(0xFF8A3A12),
    )

    fun triggerPulse(source: String) {
        val nowRealtimeMs = SystemClock.elapsedRealtime()
        pulseIntervalMs = if (lastPulseRealtimeMs > 0L) {
            (nowRealtimeMs - lastPulseRealtimeMs).toInt()
        } else {
            0
        }
        lastPulseRealtimeMs = nowRealtimeMs
        pulseId = LatencyLabController.triggerPulse(source)
        pulseSource = source
        pulseAccentIndex = (pulseAccentIndex + 1) % accentPalette.size
        pendingTapNs = System.nanoTime()
    }

    LaunchedEffect(autoPulse) {
        if (!autoPulse) {
            return@LaunchedEffect
        }

        while (true) {
            delay(700)
            triggerPulse("AUTO")
        }
    }

    LaunchedEffect(pulseId) {
        val tapNs = pendingTapNs ?: return@LaunchedEffect
        withFrameNanos { frameNs ->
            val measuredMs = ((frameNs - tapNs).coerceAtLeast(0L) / 1_000_000.0).roundToInt()
            tapToUiMs = measuredMs
            bestTapToUiMs = bestTapToUiMs?.let { minOf(it, measuredMs) } ?: measuredMs
            worstTapToUiMs = worstTapToUiMs?.let { maxOf(it, measuredMs) } ?: measuredMs
            LatencyLabController.markPulseVisible(
                pulseId = pulseId,
                source = pulseSource,
                framePresentationTimeUs = frameNs / 1_000L,
                tapToUiMs = measuredMs,
            )
        }
        flashActive = true
        delay(140)
        flashActive = false
    }

    val pulseColor = accentPalette[pulseAccentIndex]
    val panelColor = if (flashActive) {
        pulseColor
    } else {
        MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.42f)
    }
    val approxSenderMs = tapToUiMs + uiState.metrics.pipelineLatencyMs

    ElevatedCard(
        colors = CardDefaults.elevatedCardColors(
            containerColor = MaterialTheme.colorScheme.surface,
        ),
    ) {
        Column(
            modifier = Modifier.padding(18.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Column(
                    modifier = Modifier.weight(1f),
                    verticalArrangement = Arrangement.spacedBy(4.dp),
                ) {
                    Text(
                        text = "Latency Lab",
                        style = MaterialTheme.typography.titleMedium,
                        fontWeight = FontWeight.SemiBold,
                    )
                    Text(
                        text = "Тапай по панели. Вспышка и счётчик попадут в стрим, а рядом будут local sender-side цифры для сравнения.",
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                Switch(
                    checked = autoPulse,
                    onCheckedChange = { autoPulse = it },
                )
            }

            Surface(
                modifier = Modifier
                    .fillMaxWidth()
                    .height(240.dp)
                    .pointerInput(autoPulse) {
                        detectTapGestures(
                            onPress = {
                                triggerPulse("TAP")
                                tryAwaitRelease()
                            },
                        )
                    },
                shape = RoundedCornerShape(24.dp),
                color = panelColor,
            ) {
                Box(
                    modifier = Modifier.fillMaxSize(),
                    contentAlignment = Alignment.Center,
                ) {
                    Column(
                        horizontalAlignment = Alignment.CenterHorizontally,
                        verticalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        Text(
                            text = if (pulseId == 0) "TAP TO FLASH" else "PULSE #$pulseId",
                            style = MaterialTheme.typography.headlineMedium,
                            fontWeight = FontWeight.Bold,
                            color = if (flashActive) Color(0xFFFAFCFF) else MaterialTheme.colorScheme.onSurface,
                        )
                        Text(
                            text = if (autoPulse) {
                                "Source: $pulseSource | auto every 700 ms"
                            } else {
                                "Source: $pulseSource | tap the panel to trigger a new flash"
                            },
                            style = MaterialTheme.typography.bodyMedium,
                            color = if (flashActive) Color(0xFFE7F0FF) else MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                        Text(
                            text = if (pulseIntervalMs > 0) {
                                "Last pulse gap: ${pulseIntervalMs} ms"
                            } else {
                                "Waiting for the first pulse"
                            },
                            style = MaterialTheme.typography.bodySmall,
                            color = if (flashActive) Color(0xFFE7F0FF) else MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
            }

            MetricRow(
                leftLabel = "Tap -> UI",
                leftValue = if (pulseId > 0) "$tapToUiMs ms" else "-",
                rightLabel = "Capture -> send",
                rightValue = "${uiState.metrics.pipelineLatencyMs} ms",
            )
            MetricRow(
                leftLabel = "Approx sender",
                leftValue = if (pulseId > 0) "$approxSenderMs ms" else "-",
                rightLabel = "FPS",
                rightValue = uiState.metrics.fps.toString(),
            )
            MetricRow(
                leftLabel = "Best / worst",
                leftValue = if (bestTapToUiMs != null && worstTapToUiMs != null) {
                    "${bestTapToUiMs} / ${worstTapToUiMs} ms"
                } else {
                    "-"
                },
                rightLabel = "Bitrate",
                rightValue = "${uiState.metrics.bitrateKbps} kbps",
            )
        }
    }
}

@Composable
private fun TouchLatencySprintCard(
    enabled: Boolean,
    serviceEnabled: Boolean,
    sprintActive: Boolean,
    onEnabledChange: (Boolean) -> Unit,
    onOpenAccessibilitySettings: () -> Unit,
) {
    ElevatedCard(
        colors = CardDefaults.elevatedCardColors(
            containerColor = MaterialTheme.colorScheme.surface,
        ),
    ) {
        Column(
            modifier = Modifier.padding(18.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text(
                text = "Touch latency sprint",
                style = MaterialTheme.typography.titleMedium,
                fontWeight = FontWeight.SemiBold,
            )
            Text(
                text = "Короткий 160 мс burst-режим после системных касаний и скролла. Sender временно сильнее прижимает latency и быстрее просит свежий кадр.",
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Column(
                    modifier = Modifier.weight(1f),
                    verticalArrangement = Arrangement.spacedBy(2.dp),
                ) {
                    Text(
                        text = if (enabled) "Sprint enabled" else "Sprint disabled",
                        style = MaterialTheme.typography.bodyLarge,
                        fontWeight = FontWeight.Medium,
                    )
                    Text(
                        text = when {
                            !serviceEnabled -> "Accessibility service is off"
                            sprintActive -> "Sprint signal is active now"
                            else -> "Accessibility service is ready"
                        },
                        style = MaterialTheme.typography.bodySmall,
                        color = when {
                            !serviceEnabled -> MaterialTheme.colorScheme.error
                            sprintActive -> Color(0xFF25C77A)
                            else -> MaterialTheme.colorScheme.onSurfaceVariant
                        },
                    )
                }
                Switch(
                    checked = enabled,
                    onCheckedChange = onEnabledChange,
                )
            }
            OutlinedButton(
                onClick = onOpenAccessibilitySettings,
            ) {
                Text(
                    text = if (serviceEnabled) {
                        "Accessibility settings"
                    } else {
                        "Enable service"
                    },
                )
            }
        }
    }
}

@Composable
private fun GamepadBoostCard(
    enabled: Boolean,
    gamepadConnected: Boolean,
    onEnabledChange: (Boolean) -> Unit,
) {
    ElevatedCard(
        colors = CardDefaults.elevatedCardColors(
            containerColor = MaterialTheme.colorScheme.surface,
        ),
    ) {
        Column(
            modifier = Modifier.padding(18.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text(
                text = "Gamepad boost",
                style = MaterialTheme.typography.titleMedium,
                fontWeight = FontWeight.SemiBold,
            )
            Text(
                text = "Если геймпад подключён, sender удерживает более жёсткий игровой latency-профиль: раньше режет хвост, даёт encoder больше headroom и сильнее приоритизирует отклик над качеством.",
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Column(
                    modifier = Modifier.weight(1f),
                    verticalArrangement = Arrangement.spacedBy(2.dp),
                ) {
                    Text(
                        text = if (enabled) "Boost enabled" else "Boost disabled",
                        style = MaterialTheme.typography.bodyLarge,
                        fontWeight = FontWeight.Medium,
                    )
                    Text(
                        text = if (gamepadConnected) {
                            "Gamepad detected"
                        } else {
                            "No gamepad detected"
                        },
                        style = MaterialTheme.typography.bodySmall,
                        color = if (gamepadConnected) Color(0xFF25C77A) else MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                Switch(
                    checked = enabled,
                    onCheckedChange = onEnabledChange,
                )
            }
        }
    }
}

@Composable
private fun AdaptiveRoiSplitStreamCard(
    enabled: Boolean,
    onEnabledChange: (Boolean) -> Unit,
) {
    ElevatedCard(
        colors = CardDefaults.elevatedCardColors(
            containerColor = MaterialTheme.colorScheme.surface,
        ),
    ) {
        Column(
            modifier = Modifier.padding(18.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text(
                text = "Adaptive ROI split-stream",
                style = MaterialTheme.typography.titleMedium,
                fontWeight = FontWeight.SemiBold,
            )
            Text(
                text = "EVRT v3 РґРµСЂР¶РёС‚ base stream РЅР° РІРµСЃСЊ СЌРєСЂР°РЅ Рё РїРѕРґРЅРёРјР°РµС‚ enhancement stream С‚РѕР»СЊРєРѕ РІРѕРєСЂСѓРі Р°РєС‚РёРІРЅРѕР№ ROI-Р·РѕРЅС‹. Р”Р»СЏ ADB/TCP РѕСЃС‚Р°С‘С‚СЃСЏ РѕРґРёРЅ base stream.",
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Column(
                    modifier = Modifier.weight(1f),
                    verticalArrangement = Arrangement.spacedBy(2.dp),
                ) {
                    Text(
                        text = if (enabled) "Split-stream enabled" else "Split-stream disabled",
                        style = MaterialTheme.typography.bodyLarge,
                        fontWeight = FontWeight.Medium,
                    )
                    Text(
                        text = if (enabled) {
                            "Base + ROI enhancement is allowed on the current transport"
                        } else {
                            "Only the base stream will be sent until this is enabled"
                        },
                        style = MaterialTheme.typography.bodySmall,
                        color = if (enabled) Color(0xFF25C77A) else MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                Switch(
                    checked = enabled,
                    onCheckedChange = onEnabledChange,
                )
            }
        }
    }
}

@Composable
private fun TransportSelector(
    selected: StreamTransport,
    enabled: Boolean,
    onSelected: (StreamTransport) -> Unit,
) {
    ElevatedCard(
        colors = CardDefaults.elevatedCardColors(
            containerColor = MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.35f),
        ),
    ) {
        Column(
            modifier = Modifier.padding(14.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Text(
                text = "Transport",
                style = MaterialTheme.typography.titleSmall,
                fontWeight = FontWeight.SemiBold,
            )
            StreamTransport.entries.forEach { transport ->
                val isSelected = transport == selected
                Surface(
                    modifier = Modifier
                        .fillMaxWidth()
                        .clickable(enabled = enabled) { onSelected(transport) },
                    shape = RoundedCornerShape(18.dp),
                    color = if (isSelected) Color(0xFF24384C) else MaterialTheme.colorScheme.surface.copy(alpha = 0.55f),
                ) {
                    Column(
                        modifier = Modifier.padding(14.dp),
                        verticalArrangement = Arrangement.spacedBy(4.dp),
                    ) {
                        Text(
                            text = transport.uiName,
                            style = MaterialTheme.typography.titleSmall,
                            fontWeight = FontWeight.SemiBold,
                            color = if (isSelected) Color(0xFFF3F9FF) else MaterialTheme.colorScheme.onSurface,
                        )
                        Text(
                            text = transport.summary,
                            style = MaterialTheme.typography.bodyMedium,
                            color = if (isSelected) Color(0xFFD4E6F8) else MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
            }
        }
    }
}

@Composable
private fun SessionStatusCard(uiState: StreamUiState) {
    val tone = when (uiState.phase) {
        StreamPhase.IDLE -> MaterialTheme.colorScheme.surface
        StreamPhase.REQUESTING_PERMISSION -> Color(0xFF153149)
        StreamPhase.STARTING -> Color(0xFF1E3A4C)
        StreamPhase.STREAMING -> Color(0xFF143D2E)
        StreamPhase.ERROR -> MaterialTheme.colorScheme.errorContainer
    }

    ElevatedCard(
        colors = CardDefaults.elevatedCardColors(containerColor = tone),
    ) {
        Column(
            modifier = Modifier.padding(18.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    Text(
                        text = "Статус",
                        style = MaterialTheme.typography.labelLarge,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    Text(
                        text = uiState.status,
                        style = MaterialTheme.typography.titleMedium,
                        fontWeight = FontWeight.SemiBold,
                    )
                }
                StatusPill(label = uiState.phase.label)
            }

            uiState.activeEndpoint?.let { endpoint ->
                Text(
                    text = "Endpoint: $endpoint",
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }

            uiState.activeCodec?.let { codec ->
                Text(
                    text = "Codec: ${codec.uiName}",
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }

            uiState.lastError?.takeIf { it.isNotBlank() }?.let { error ->
                Text(
                    text = error,
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.error,
                )
            }

            StatGrid(uiState = uiState)
        }
    }
}

@Composable
private fun StatusPill(label: String) {
    Surface(
        shape = RoundedCornerShape(999.dp),
        color = MaterialTheme.colorScheme.onBackground.copy(alpha = 0.08f),
    ) {
        Text(
            text = label,
            modifier = Modifier.padding(horizontal = 12.dp, vertical = 6.dp),
            style = MaterialTheme.typography.labelLarge,
            fontWeight = FontWeight.Medium,
        )
    }
}

@Composable
private fun StatGrid(uiState: StreamUiState) {
    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
        MetricRow(
            leftLabel = "Preset",
            leftValue = uiState.activePreset?.uiName ?: "Не выбран",
            rightLabel = "Разрешение",
            rightValue = uiState.metrics.resolutionLabel,
        )
        MetricRow(
            leftLabel = "FPS",
            leftValue = uiState.metrics.fps.toString(),
            rightLabel = "Битрейт",
            rightValue = "${uiState.metrics.bitrateKbps} kbps",
        )
        MetricRow(
            leftLabel = "Пайплайн",
            leftValue = "${uiState.metrics.pipelineLatencyMs} ms",
            rightLabel = "Фреймы",
            rightValue = uiState.metrics.framesSent.toString(),
        )
        MetricRow(
            leftLabel = "UDP пакеты",
            leftValue = uiState.metrics.packetsSent.toString(),
            rightLabel = "Дропы",
            rightValue = uiState.metrics.droppedFrames.toString(),
        )
    }
}

@Composable
private fun MetricRow(
    leftLabel: String,
    leftValue: String,
    rightLabel: String,
    rightValue: String,
) {
    Row(
        modifier = Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        MetricTile(
            modifier = Modifier.weight(1f),
            label = leftLabel,
            value = leftValue,
        )
        MetricTile(
            modifier = Modifier.weight(1f),
            label = rightLabel,
            value = rightValue,
        )
    }
}

@Composable
private fun MetricTile(
    modifier: Modifier = Modifier,
    label: String,
    value: String,
) {
    Surface(
        modifier = modifier,
        shape = RoundedCornerShape(18.dp),
        color = MaterialTheme.colorScheme.onBackground.copy(alpha = 0.05f),
    ) {
        Column(
            modifier = Modifier.padding(horizontal = 14.dp, vertical = 12.dp),
            verticalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            Text(
                text = label,
                style = MaterialTheme.typography.labelMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Text(
                text = value,
                style = MaterialTheme.typography.titleMedium,
                fontWeight = FontWeight.SemiBold,
            )
        }
    }
}

@Composable
private fun CodecSelector(
    codecs: List<VideoCodec>,
    selected: VideoCodec,
    enabled: Boolean,
    onSelected: (VideoCodec) -> Unit,
) {
    ElevatedCard(
        colors = CardDefaults.elevatedCardColors(
            containerColor = MaterialTheme.colorScheme.surface,
        ),
    ) {
        Column(
            modifier = Modifier.padding(18.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text(
                text = "Codec",
                style = MaterialTheme.typography.titleMedium,
                fontWeight = FontWeight.SemiBold,
            )

            codecs.forEach { codec ->
                val isSelected = codec == selected
                Surface(
                    modifier = Modifier
                        .fillMaxWidth()
                        .clickable(enabled = enabled) { onSelected(codec) },
                    shape = RoundedCornerShape(20.dp),
                    color = if (isSelected) {
                        Color(0xFF16333C)
                    } else {
                        MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.45f)
                    },
                ) {
                    Column(
                        modifier = Modifier.padding(16.dp),
                        verticalArrangement = Arrangement.spacedBy(4.dp),
                    ) {
                        Text(
                            text = codec.uiName,
                            style = MaterialTheme.typography.titleSmall,
                            fontWeight = FontWeight.SemiBold,
                            color = if (isSelected) Color(0xFFF3F9FF) else MaterialTheme.colorScheme.onSurface,
                        )
                        Text(
                            text = codec.summary,
                            style = MaterialTheme.typography.bodyMedium,
                            color = if (isSelected) Color(0xFFD4E6F8) else MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
            }
        }
    }
}

@Composable
private fun AudioSelectorCard(
    enabled: Boolean,
    checked: Boolean,
    supported: Boolean,
    onCheckedChange: (Boolean) -> Unit,
) {
    ElevatedCard(
        colors = CardDefaults.elevatedCardColors(
            containerColor = MaterialTheme.colorScheme.surface,
        ),
    ) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(18.dp),
            horizontalArrangement = Arrangement.SpaceBetween,
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Column(
                modifier = Modifier.weight(1f),
                verticalArrangement = Arrangement.spacedBy(4.dp),
            ) {
                Text(
                    text = "Device audio",
                    style = MaterialTheme.typography.titleMedium,
                    fontWeight = FontWeight.SemiBold,
                )
                Text(
                    text = if (supported) {
                        "Capture game audio from Android playback path."
                    } else {
                        "Device audio capture requires Android 10 or newer."
                    },
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            Switch(
                checked = checked,
                enabled = enabled,
                onCheckedChange = onCheckedChange,
            )
        }
    }
}

@Composable
private fun QualityPresetSelector(
    selected: QualityPreset,
    enabled: Boolean,
    onSelected: (QualityPreset) -> Unit,
) {
    val visiblePresets = remember {
        listOf(
            QualityPreset.WI_FI_GAMING,
            QualityPreset.TOURNAMENT_FIGHTER,
            QualityPreset.INSTANT_PLAY,
        )
    }
    ElevatedCard(
        colors = CardDefaults.elevatedCardColors(
            containerColor = MaterialTheme.colorScheme.surface,
        ),
    ) {
        Column(
            modifier = Modifier.padding(18.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text(
                text = "Профиль качества",
                style = MaterialTheme.typography.titleMedium,
                fontWeight = FontWeight.SemiBold,
            )

            visiblePresets.forEach { preset ->
                val isSelected = preset == selected
                Surface(
                    modifier = Modifier
                        .fillMaxWidth()
                        .clickable(enabled = enabled) { onSelected(preset) },
                    shape = RoundedCornerShape(20.dp),
                    color = if (isSelected) Color(0xFF13324A) else MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.45f),
                ) {
                    Column(
                        modifier = Modifier.padding(16.dp),
                        verticalArrangement = Arrangement.spacedBy(4.dp),
                    ) {
                        Text(
                            text = preset.uiName,
                            style = MaterialTheme.typography.titleSmall,
                            fontWeight = FontWeight.SemiBold,
                            color = if (isSelected) Color(0xFFF3F9FF) else MaterialTheme.colorScheme.onSurface,
                        )
                        Text(
                            text = preset.summary,
                            style = MaterialTheme.typography.bodyMedium,
                            color = if (isSelected) Color(0xFFD4E6F8) else MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
            }
        }
    }
}

@Composable
private fun AdaptationModeSelector(
    selected: AdaptationMode,
    enabled: Boolean,
    onSelected: (AdaptationMode) -> Unit,
) {
    ElevatedCard(
        colors = CardDefaults.elevatedCardColors(
            containerColor = MaterialTheme.colorScheme.surface,
        ),
    ) {
        Column(
            modifier = Modifier.padding(18.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text(
                text = "Adaptation",
                style = MaterialTheme.typography.titleMedium,
                fontWeight = FontWeight.SemiBold,
            )

            AdaptationMode.entries.forEach { mode ->
                val isSelected = mode == selected
                Surface(
                    modifier = Modifier
                        .fillMaxWidth()
                        .clickable(enabled = enabled) { onSelected(mode) },
                    shape = RoundedCornerShape(20.dp),
                    color = if (isSelected) Color(0xFF17351D) else MaterialTheme.colorScheme.surfaceVariant.copy(alpha = 0.45f),
                ) {
                    Column(
                        modifier = Modifier.padding(16.dp),
                        verticalArrangement = Arrangement.spacedBy(4.dp),
                    ) {
                        Text(
                            text = mode.uiName,
                            style = MaterialTheme.typography.titleSmall,
                            fontWeight = FontWeight.SemiBold,
                            color = if (isSelected) Color(0xFFF3F9FF) else MaterialTheme.colorScheme.onSurface,
                        )
                        Text(
                            text = mode.summary,
                            style = MaterialTheme.typography.bodyMedium,
                            color = if (isSelected) Color(0xFFD8EFD8) else MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
            }
        }
    }
}
