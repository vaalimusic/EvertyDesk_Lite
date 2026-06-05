package com.everty.evertydesk.client

data class AndroidConnectionState(
    val status: String = "Готов к исходящему подключению",
    val stage: String = "Idle",
    val connected: Boolean = false,
    val canOpenViewer: Boolean = false,
    val logs: List<String> = listOf("Android outgoing client started"),
)
