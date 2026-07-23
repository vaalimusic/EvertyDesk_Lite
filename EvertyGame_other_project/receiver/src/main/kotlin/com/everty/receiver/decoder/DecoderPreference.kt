package com.everty.receiver.decoder

enum class DecoderPreference(
    val uiLabel: String,
) {
    AUTO("Auto"),
    D3D11VA("D3D11VA"),
    DXVA2("DXVA2"),
    NVDEC_CUDA("NVDEC / CUDA"),
    SOFTWARE("Software");

    override fun toString(): String = uiLabel
}
