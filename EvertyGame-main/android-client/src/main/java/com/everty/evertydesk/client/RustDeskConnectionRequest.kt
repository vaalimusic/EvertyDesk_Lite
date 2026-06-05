package com.everty.evertydesk.client

data class RustDeskConnectionRequest(
    val remoteId: String,
    val password: String,
    val idServer: String,
    val relayServer: String,
    val publicKey: String,
    val targetFps: Int,
    val codec: String,
)
