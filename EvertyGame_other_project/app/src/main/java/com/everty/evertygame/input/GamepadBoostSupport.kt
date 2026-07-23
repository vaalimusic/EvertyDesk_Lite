package com.everty.evertygame.input

import android.content.Context
import android.hardware.input.InputManager
import android.os.Handler
import android.view.InputDevice

object GamepadBoostSupport {
    fun hasConnectedGamepad(context: Context): Boolean {
        return InputDevice.getDeviceIds()
            .asIterable()
            .mapNotNull(InputDevice::getDevice)
            .any(::isGamepad)
    }

    private fun isGamepad(device: InputDevice?): Boolean {
        device ?: return false
        val sources = device.sources
        val hasGamepadSource =
            (sources and InputDevice.SOURCE_GAMEPAD) == InputDevice.SOURCE_GAMEPAD ||
                (sources and InputDevice.SOURCE_JOYSTICK) == InputDevice.SOURCE_JOYSTICK
        return hasGamepadSource && !device.isVirtual
    }

    class Monitor(
        context: Context,
        private val handler: Handler?,
        private val onStateChanged: (Boolean) -> Unit,
    ) : AutoCloseable {
        private val appContext = context.applicationContext
        private val inputManager = appContext.getSystemService(InputManager::class.java)
        private val listener = object : InputManager.InputDeviceListener {
            override fun onInputDeviceAdded(deviceId: Int) = dispatch()

            override fun onInputDeviceRemoved(deviceId: Int) = dispatch()

            override fun onInputDeviceChanged(deviceId: Int) = dispatch()
        }

        private var started = false

        fun start() {
            if (started || inputManager == null) {
                dispatch()
                return
            }
            started = true
            inputManager.registerInputDeviceListener(listener, handler)
            dispatch()
        }

        private fun dispatch() {
            onStateChanged(hasConnectedGamepad(appContext))
        }

        override fun close() {
            if (!started || inputManager == null) {
                return
            }
            started = false
            inputManager.unregisterInputDeviceListener(listener)
        }
    }
}
