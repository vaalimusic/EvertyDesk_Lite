package com.everty.evertygame.stream

enum class StreamTransport(
    val uiName: String,
    val summary: String,
    val hostLabel: String,
    val portLabel: String,
    val sessionTag: String,
) {
    UDP(
        uiName = "UDP / Wi-Fi",
        summary = "Default low-latency LAN path over UDP.",
        hostLabel = "PC IP or hostname",
        portLabel = "UDP port",
        sessionTag = "EVRT_REALTIME_V2_UDP",
    ),
    ADB_TUNNEL_TCP(
        uiName = "ADB tunnel / TCP",
        summary = "USB path via `adb reverse tcp:5001 tcp:5001`. Use host `127.0.0.1` on the phone.",
        hostLabel = "ADB host (usually 127.0.0.1)",
        portLabel = "TCP port",
        sessionTag = "EVRT_REALTIME_V2_TCP_ADB",
    );
}
