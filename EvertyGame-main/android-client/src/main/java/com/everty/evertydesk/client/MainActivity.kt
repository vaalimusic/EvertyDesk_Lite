package com.everty.evertydesk.client

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.unit.dp

private enum class ClientMode(val label: String) {
    Screen("Экран"),
    Terminal("Терминал"),
    Files("Файлы"),
}

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent {
            EvertyDeskClientTheme {
                EvertyDeskClientApp()
            }
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class, ExperimentalLayoutApi::class)
@Composable
private fun EvertyDeskClientApp() {
    val controller = remember { AndroidConnectionController() }
    var state by remember { mutableStateOf(AndroidConnectionState()) }
    var remoteId by remember { mutableStateOf("") }
    var password by remember { mutableStateOf("") }
    var showPassword by remember { mutableStateOf(false) }
    var idServer by remember { mutableStateOf("edesk.server1.everty.ru") }
    var relayServer by remember { mutableStateOf("edesk.server1.everty.ru") }
    var publicKey by remember {
        mutableStateOf("MrGdbay3g8Qr84YYnxr4qLjw5zLWM1oAOdfehbBnlRs=")
    }
    var targetFps by remember { mutableStateOf("60") }
    var codec by remember { mutableStateOf("Auto") }
    var mode by remember { mutableStateOf(ClientMode.Screen) }

    Scaffold(
        topBar = {
            TopAppBar(
                title = {
                    Column {
                        Text("EvertyDesk Client", fontWeight = FontWeight.SemiBold)
                        Text(state.status, style = MaterialTheme.typography.bodySmall)
                    }
                },
            )
        },
    ) { padding ->
        Column(
            modifier = Modifier
                .fillMaxSize()
                .background(Color(0xFFF4F6F8))
                .verticalScroll(rememberScrollState())
                .padding(padding)
                .padding(14.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            SurfaceBlock {
                Text("Исходящее подключение", style = MaterialTheme.typography.titleMedium)
                Spacer(Modifier.height(10.dp))
                OutlinedTextField(
                    value = remoteId,
                    onValueChange = { remoteId = it.filter(Char::isDigit) },
                    label = { Text("ID удаленной машины") },
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )
                Spacer(Modifier.height(8.dp))
                OutlinedTextField(
                    value = password,
                    onValueChange = { password = it },
                    label = { Text("Пароль или подтверждение на хосте") },
                    singleLine = true,
                    visualTransformation = if (showPassword) {
                        VisualTransformation.None
                    } else {
                        PasswordVisualTransformation()
                    },
                    trailingIcon = {
                        OutlinedButton(onClick = { showPassword = !showPassword }) {
                            Text(if (showPassword) "Скрыть" else "Показать")
                        }
                    },
                    modifier = Modifier.fillMaxWidth(),
                )
                Spacer(Modifier.height(10.dp))
                FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    ClientMode.entries.forEach { item ->
                        FilterChip(
                            selected = mode == item,
                            onClick = { mode = item },
                            label = { Text(item.label) },
                        )
                    }
                }
                Spacer(Modifier.height(10.dp))
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Button(
                        onClick = {
                            state = controller.start(
                                RustDeskConnectionRequest(
                                    remoteId = remoteId,
                                    password = password,
                                    idServer = idServer,
                                    relayServer = relayServer,
                                    publicKey = publicKey,
                                    targetFps = targetFps.toIntOrNull()?.coerceIn(5, 60) ?: 60,
                                    codec = codec,
                                ),
                            )
                        },
                        enabled = remoteId.isNotBlank(),
                    ) {
                        Text("Подключиться")
                    }
                    OutlinedButton(onClick = { state = controller.disconnect(state) }) {
                        Text("Отключить")
                    }
                }
            }

            SurfaceBlock {
                Text("Быстрые функции", style = MaterialTheme.typography.titleMedium)
                Spacer(Modifier.height(10.dp))
                FlowRow(
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                    verticalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    QuickAction("Монитор", "переключение дисплея")
                    QuickAction("Буфер", "вставка текста")
                    QuickAction("Fullscreen", "полный экран")
                    QuickAction("Ctrl+Alt+Del", "системная команда")
                }
            }

            SurfaceBlock {
                Text("Сеть и видео", style = MaterialTheme.typography.titleMedium)
                Spacer(Modifier.height(10.dp))
                OutlinedTextField(
                    value = idServer,
                    onValueChange = { idServer = it.trim() },
                    label = { Text("ID server") },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )
                Spacer(Modifier.height(8.dp))
                OutlinedTextField(
                    value = relayServer,
                    onValueChange = { relayServer = it.trim() },
                    label = { Text("Relay server") },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )
                Spacer(Modifier.height(8.dp))
                OutlinedTextField(
                    value = publicKey,
                    onValueChange = { publicKey = it.trim() },
                    label = { Text("Public key") },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth(),
                )
                Spacer(Modifier.height(8.dp))
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    OutlinedTextField(
                        value = targetFps,
                        onValueChange = { targetFps = it.filter(Char::isDigit).take(2) },
                        label = { Text("FPS") },
                        singleLine = true,
                        modifier = Modifier.weight(1f),
                    )
                    OutlinedTextField(
                        value = codec,
                        onValueChange = { codec = it.take(12) },
                        label = { Text("Кодек") },
                        singleLine = true,
                        modifier = Modifier.weight(1f),
                    )
                }
            }

            SurfaceBlock {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    StatusDot(active = state.connected)
                    Spacer(Modifier.width(8.dp))
                    Column {
                        Text(state.stage, fontWeight = FontWeight.SemiBold)
                        Text(state.status, style = MaterialTheme.typography.bodySmall)
                    }
                }
                Spacer(Modifier.height(10.dp))
                state.logs.forEach { line ->
                    Text(line, style = MaterialTheme.typography.bodySmall)
                }
            }
        }
    }
}

@Composable
private fun EvertyDeskClientTheme(content: @Composable () -> Unit) {
    MaterialTheme(
        colorScheme = lightColorScheme(
            primary = Color(0xFF176D5B),
            secondary = Color(0xFF334155),
            background = Color(0xFFF4F6F8),
            surface = Color.White,
        ),
        content = content,
    )
}

@Composable
private fun SurfaceBlock(content: @Composable ColumnScope.() -> Unit) {
    Surface(
        modifier = Modifier.fillMaxWidth(),
        shape = RoundedCornerShape(8.dp),
        color = Color.White,
        tonalElevation = 1.dp,
        shadowElevation = 1.dp,
    ) {
        Column(Modifier.padding(14.dp), content = content)
    }
}

@Composable
private fun QuickAction(title: String, detail: String) {
    Box(
        modifier = Modifier
            .border(1.dp, Color(0xFFD5DDE5), RoundedCornerShape(8.dp))
            .padding(horizontal = 12.dp, vertical = 10.dp),
    ) {
        Column {
            Text(title, fontWeight = FontWeight.SemiBold)
            Text(detail, style = MaterialTheme.typography.bodySmall, color = Color(0xFF64748B))
        }
    }
}

@Composable
private fun StatusDot(active: Boolean) {
    Box(
        Modifier
            .size(10.dp)
            .background(
                if (active) Color(0xFF16A34A) else Color(0xFF94A3B8),
                RoundedCornerShape(50),
            ),
    )
}
