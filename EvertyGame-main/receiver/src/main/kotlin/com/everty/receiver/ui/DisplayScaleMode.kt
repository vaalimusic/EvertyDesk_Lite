package com.everty.receiver.ui

enum class DisplayScaleMode(
    private val label: String,
) {
    FIT("Fit"),
    FILL("Fill"),
    STRETCH("Stretch");

    override fun toString(): String = label
}
