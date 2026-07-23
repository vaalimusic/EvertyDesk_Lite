package com.everty.evertygame.stream

import java.io.Closeable
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetSocketAddress
import java.util.concurrent.atomic.AtomicBoolean

class UdpSender(
    host: String,
    port: Int,
    private val onControlPacket: (ByteArray) -> Unit = {},
    private val onTransportError: (String) -> Unit = {},
) : PacketSender {
    private val running = AtomicBoolean(true)
    private val socket = DatagramSocket().apply {
        sendBufferSize = 1 shl 20
        receiveBufferSize = 1 shl 20
        trafficClass = 0x10
        connect(InetSocketAddress(host, port))
        soTimeout = 0
    }
    private val receiveThread = Thread({
        val buffer = ByteArray(TransportProtocol.MAX_PACKET_SIZE)
        while (running.get()) {
            try {
                val packet = DatagramPacket(buffer, buffer.size)
                socket.receive(packet)
                if (packet.length < TransportProtocol.HEADER_SIZE) {
                    continue
                }

                val type = packet.data[5]
                if (type != TransportProtocol.TYPE_CONTROL) {
                    continue
                }

                val payloadSize = packet.length - TransportProtocol.HEADER_SIZE
                if (payloadSize <= 0) {
                    continue
                }

                val payload = packet.data.copyOfRange(
                    TransportProtocol.HEADER_SIZE,
                    TransportProtocol.HEADER_SIZE + payloadSize,
                )
                onControlPacket(payload)
            } catch (t: Throwable) {
                if (running.get()) {
                    onTransportError(t.message ?: "Control receive loop failed")
                }
            }
        }
    }, "EvertySenderControlRx").apply {
        isDaemon = true
        start()
    }

    @Synchronized
    override fun send(packet: ByteArray) {
        socket.send(DatagramPacket(packet, packet.size))
    }

    override fun close() {
        if (!running.compareAndSet(true, false)) {
            return
        }
        socket.close()
        receiveThread.join(1_000)
    }
}
