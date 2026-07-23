package com.everty.evertygame.stream

import java.nio.ByteBuffer
import java.util.concurrent.atomic.AtomicBoolean

class NativeUdpSender(
    host: String,
    port: Int,
    private val onControlPacket: (ByteArray) -> Unit = {},
    private val onTransportError: (String) -> Unit = {},
) : PacketSender, FastVideoFrameSender {
    private val running = AtomicBoolean(true)
    private val nativeHandle = nativeCreate(host, port)
    private val receiveThread = Thread({
        while (running.get()) {
            try {
                val payload = nativeReceiveControlPayload(nativeHandle) ?: continue
                onControlPacket(payload)
            } catch (t: Throwable) {
                if (running.get()) {
                    onTransportError(t.message ?: "Native UDP control receive loop failed")
                }
            }
        }
    }, "EvertyNativeUdpControlRx").apply {
        isDaemon = true
        start()
    }

    override fun send(packet: ByteArray) {
        nativeSendPacket(nativeHandle, packet, packet.size)
    }

    override fun sendVideoFrameDirect(
        frameId: Int,
        presentationTimeUs: Long,
        isKeyFrame: Boolean,
        payloadBuffer: ByteBuffer,
        payloadOffset: Int,
        payloadSize: Int,
    ): Int {
        return nativeSendVideoFrame(
            nativeHandle = nativeHandle,
            frameId = frameId,
            presentationTimeUs = presentationTimeUs,
            isKeyFrame = isKeyFrame,
            payloadBuffer = payloadBuffer,
            payloadOffset = payloadOffset,
            payloadSize = payloadSize,
        )
    }

    override fun close() {
        if (!running.compareAndSet(true, false)) {
            return
        }
        nativeShutdown(nativeHandle)
        receiveThread.join(1_000)
        nativeDestroy(nativeHandle)
    }

    private external fun nativeCreate(host: String, port: Int): Long

    private external fun nativeSendPacket(
        nativeHandle: Long,
        packet: ByteArray,
        packetSize: Int,
    )

    private external fun nativeSendVideoFrame(
        nativeHandle: Long,
        frameId: Int,
        presentationTimeUs: Long,
        isKeyFrame: Boolean,
        payloadBuffer: ByteBuffer,
        payloadOffset: Int,
        payloadSize: Int,
    ): Int

    private external fun nativeReceiveControlPayload(nativeHandle: Long): ByteArray?

    private external fun nativeShutdown(nativeHandle: Long)

    private external fun nativeDestroy(nativeHandle: Long)

    companion object {
        init {
            System.loadLibrary("evertysender")
        }
    }
}
