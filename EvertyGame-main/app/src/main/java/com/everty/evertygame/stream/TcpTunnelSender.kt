package com.everty.evertygame.stream

import java.io.EOFException
import java.io.InputStream
import java.net.InetSocketAddress
import java.net.Socket
import java.util.concurrent.atomic.AtomicBoolean

class TcpTunnelSender(
    host: String,
    port: Int,
    private val onControlPacket: (ByteArray) -> Unit = {},
    private val onTransportError: (String) -> Unit = {},
) : PacketSender {
    private val running = AtomicBoolean(true)
    private val socket = Socket().apply {
        tcpNoDelay = true
        sendBufferSize = 1 shl 20
        receiveBufferSize = 1 shl 20
        trafficClass = 0x10
        keepAlive = true
        connect(InetSocketAddress(host, port), 4_000)
        soTimeout = 0
    }
    private val input = socket.getInputStream()
    private val output = socket.getOutputStream()
    private val receiveThread = Thread({
        try {
            while (running.get()) {
                val packetLength = readLengthPrefix(input)
                if (packetLength < TransportProtocol.HEADER_SIZE || packetLength > MAX_FRAMED_PACKET_SIZE) {
                    throw IllegalStateException("ADB tunnel packet length $packetLength is invalid")
                }

                val packet = ByteArray(packetLength)
                readFully(input, packet, packetLength)
                if (packet[5] != TransportProtocol.TYPE_CONTROL) {
                    continue
                }

                val payloadSize = packetLength - TransportProtocol.HEADER_SIZE
                if (payloadSize <= 0) {
                    continue
                }

                onControlPacket(
                    packet.copyOfRange(
                        TransportProtocol.HEADER_SIZE,
                        TransportProtocol.HEADER_SIZE + payloadSize,
                    ),
                )
            }
        } catch (t: Throwable) {
            if (running.get()) {
                onTransportError(t.message ?: "ADB tunnel control receive loop failed")
            }
        }
    }, "EvertySenderAdbTcpRx").apply {
        isDaemon = true
        start()
    }

    @Synchronized
    override fun send(packet: ByteArray) {
        writeLengthPrefix(output, packet.size)
        output.write(packet)
    }

    override fun close() {
        if (!running.compareAndSet(true, false)) {
            return
        }
        runCatching { socket.close() }
        receiveThread.join(1_000)
    }

    private fun readLengthPrefix(input: InputStream): Int {
        val header = ByteArray(4)
        readFully(input, header, header.size)
        return ((header[0].toInt() and 0xFF) shl 24) or
            ((header[1].toInt() and 0xFF) shl 16) or
            ((header[2].toInt() and 0xFF) shl 8) or
            (header[3].toInt() and 0xFF)
    }

    private fun writeLengthPrefix(output: java.io.OutputStream, length: Int) {
        output.write(
            byteArrayOf(
                ((length ushr 24) and 0xFF).toByte(),
                ((length ushr 16) and 0xFF).toByte(),
                ((length ushr 8) and 0xFF).toByte(),
                (length and 0xFF).toByte(),
            ),
        )
    }

    private fun readFully(input: InputStream, buffer: ByteArray, length: Int) {
        var offset = 0
        while (offset < length) {
            val bytesRead = input.read(buffer, offset, length - offset)
            if (bytesRead < 0) {
                throw EOFException("ADB tunnel closed")
            }
            offset += bytesRead
        }
    }

    private companion object {
        const val MAX_FRAMED_PACKET_SIZE = 64 * 1024
    }
}
