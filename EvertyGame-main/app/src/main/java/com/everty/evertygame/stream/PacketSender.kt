package com.everty.evertygame.stream

import java.io.Closeable

interface PacketSender : Closeable {
    fun send(packet: ByteArray)
}
