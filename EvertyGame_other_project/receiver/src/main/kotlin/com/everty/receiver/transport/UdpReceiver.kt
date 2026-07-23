package com.everty.receiver.transport

import java.io.Closeable
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetSocketAddress
import java.net.SocketException
import java.util.concurrent.atomic.AtomicBoolean

class UdpReceiver(
    private val port: Int,
    private val onPacket: (UdpPacket, InetSocketAddress) -> Unit,
    private val onError: (String) -> Unit,
) : Closeable {
    private val running = AtomicBoolean(false)
    private var socket: DatagramSocket? = null
    private var thread: Thread? = null

    fun start() {
        if (!running.compareAndSet(false, true)) {
            return
        }

        val receiverSocket = DatagramSocket(port).apply {
            receiveBufferSize = 1 shl 20
            soTimeout = 0
        }
        socket = receiverSocket

        thread = Thread({
            val buffer = ByteArray(TransportProtocol.MAX_PACKET_SIZE)
            while (running.get()) {
                try {
                    val packet = DatagramPacket(buffer, buffer.size)
                    receiverSocket.receive(packet)
                    val parsed = ProtocolParser.parse(packet.data, packet.length)
                    if (parsed != null) {
                        onPacket(
                            parsed,
                            InetSocketAddress(packet.address, packet.port),
                        )
                    }
                } catch (_: SocketException) {
                    break
                } catch (t: Throwable) {
                    if (running.get()) {
                        onError(t.message ?: "UDP receive loop failed")
                    }
                }
            }
        }, "EvertyUdpReceiver").apply {
            isDaemon = true
            start()
        }
    }

    @Synchronized
    fun send(bytes: ByteArray, target: InetSocketAddress) {
        val activeSocket = socket ?: return
        val packet = DatagramPacket(
            bytes,
            bytes.size,
            target.address,
            target.port,
        )
        activeSocket.send(packet)
    }

    override fun close() {
        if (!running.compareAndSet(true, false)) {
            return
        }
        socket?.close()
        thread?.join(1_000)
        socket = null
        thread = null
    }
}
