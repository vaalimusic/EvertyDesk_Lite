package com.everty.evertygame.stream

enum class AdaptationMode(
    val uiName: String,
    val summary: String,
) {
    AUTO_BALANCED(
        uiName = "Auto",
        summary = "Receiver feedback adjusts bitrate to balance quality and latency.",
    ),
    LOCK_QUALITY(
        uiName = "Fixed Quality",
        summary = "Keeps bitrate high and avoids aggressive quality drops.",
    ),
    WI_FI_LAN_TURBO(
        uiName = "Wi-Fi / LAN Turbo",
        summary = "Uses more 5 GHz / Wi-Fi 6 or LAN headroom with larger burst bitrate and gentler quality cuts.",
    ),
    LOWEST_LATENCY(
        uiName = "Lowest Latency",
        summary = "Drops bitrate faster to protect responsiveness on weaker receivers.",
    );
}
