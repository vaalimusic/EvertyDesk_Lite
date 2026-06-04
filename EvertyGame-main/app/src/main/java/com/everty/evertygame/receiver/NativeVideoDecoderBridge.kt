package com.everty.evertygame.receiver

import android.view.Surface

internal class NativeVideoDecoderBridge private constructor(
    private val nativeHandle: Long,
    val decoderPath: String,
) : AutoCloseable {
    data class DecodeResult(
        val statusCode: Int,
        val renderedFrames: Int,
        val lastRenderedPtsUs: Long,
        val outputWidth: Int,
        val outputHeight: Int,
    ) {
        val needsDrop: Boolean
            get() = statusCode < 0
    }

    fun decodeAccessUnit(accessUnit: ByteArray, presentationTimeUs: Long): DecodeResult {
        val result = nativeDecodeAccessUnit(nativeHandle, accessUnit, accessUnit.size, presentationTimeUs)
        return DecodeResult(
            statusCode = result[0].toInt(),
            renderedFrames = result[1].toInt(),
            lastRenderedPtsUs = result[2],
            outputWidth = result[3].toInt(),
            outputHeight = result[4].toInt(),
        )
    }

    override fun close() {
        nativeReleaseDecoder(nativeHandle)
    }

    private external fun nativeDecodeAccessUnit(
        nativeHandle: Long,
        accessUnit: ByteArray,
        accessUnitSize: Int,
        presentationTimeUs: Long,
    ): LongArray

    private external fun nativeReleaseDecoder(nativeHandle: Long)

    companion object {
        init {
            System.loadLibrary("evertysender")
        }

        fun create(
            codecMime: String,
            width: Int,
            height: Int,
            surface: Surface,
            codecSpecificData: List<ByteArray>,
            codecName: String?,
        ): NativeVideoDecoderBridge {
            val nativeHandle = nativeCreateDecoder(
                codecMime = codecMime,
                width = width,
                height = height,
                surface = surface,
                codecSpecificData = codecSpecificData.toTypedArray(),
                codecName = codecName,
            )
            val decoderPath = nativeGetDecoderPath(nativeHandle)
            return NativeVideoDecoderBridge(nativeHandle = nativeHandle, decoderPath = decoderPath)
        }

        @JvmStatic
        private external fun nativeCreateDecoder(
            codecMime: String,
            width: Int,
            height: Int,
            surface: Surface,
            codecSpecificData: Array<ByteArray>,
            codecName: String?,
        ): Long

        @JvmStatic
        private external fun nativeGetDecoderPath(nativeHandle: Long): String
    }
}
