package com.everty.evertygame.stream

import java.nio.ByteBuffer

interface FastVideoFrameSender {
    fun sendVideoFrameDirect(
        frameId: Int,
        presentationTimeUs: Long,
        isKeyFrame: Boolean,
        payloadBuffer: ByteBuffer,
        payloadOffset: Int,
        payloadSize: Int,
    ): Int
}
