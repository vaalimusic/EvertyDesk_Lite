package com.everty.evertydesk.client

class AndroidConnectionController {
    fun start(request: RustDeskConnectionRequest): AndroidConnectionState {
        val normalizedId = request.remoteId.filter(Char::isDigit)
        if (normalizedId.isBlank()) {
            return AndroidConnectionState(
                status = "Введите ID удаленной машины",
                stage = "Validation",
                logs = listOf("Validation failed: empty remote id"),
            )
        }

        val logs = buildList {
            add("Session requested for $normalizedId")
            add("5% - Validating input")
            add("15% - Server: ${request.idServer}")
            add("30% - Relay: ${request.relayServer}")
            add("45% - Codec preference: ${request.codec}, ${request.targetFps} fps")
            add("60% - RustDesk transport module is the next integration step")
        }

        return AndroidConnectionState(
            status = "Каркас подключения готов: следующий шаг - общий transport",
            stage = "Transport scaffold",
            connected = false,
            canOpenViewer = false,
            logs = logs,
        )
    }

    fun disconnect(previous: AndroidConnectionState): AndroidConnectionState {
        return previous.copy(
            status = "Отключено",
            stage = "Idle",
            connected = false,
            canOpenViewer = false,
            logs = previous.logs + "Disconnected",
        )
    }
}
