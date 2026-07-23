package com.everty.receiver.ui

import com.everty.receiver.ReceiverSnapshot
import java.awt.Color
import java.awt.Dimension
import java.awt.GraphicsEnvironment
import java.awt.GraphicsDevice
import java.awt.Rectangle
import java.awt.event.InputEvent
import java.awt.event.KeyEvent
import java.awt.image.BufferedImage
import java.awt.event.MouseAdapter
import java.awt.event.MouseEvent
import javax.swing.AbstractAction
import javax.swing.JComponent
import javax.swing.JFrame
import javax.swing.KeyStroke
import org.bytedeco.javacv.CanvasFrame
import org.bytedeco.javacv.Frame
import org.bytedeco.javacv.GLCanvasFrame
import org.bytedeco.javacv.Java2DFrameConverter

class LowLatencyGameView {
    private var window: CanvasFrame? = null
    private var backendLabel: String = "Closed"
    private var fullscreen = false
    private var fullscreenDevice: GraphicsDevice? = null
    private var exclusiveFullscreen = false
    private var restoreBounds: Rectangle? = null
    private var restoreExtendedState: Int = JFrame.NORMAL
    private var lastStreamSize: Dimension? = null
    private val converter = Java2DFrameConverter()

    fun isVisible(): Boolean = window?.isVisible == true

    fun backendLabel(): String = backendLabel

    fun toggleVisibility() {
        if (isVisible()) {
            hide()
        } else {
            openOrFocus()
        }
    }

    fun openOrFocus() {
        val existing = window
        if (existing != null) {
            existing.isVisible = true
            existing.toFront()
            existing.requestFocus()
            return
        }

        val created = createWindow()
        configureWindow(created)
        window = created
        created.isVisible = true
        created.toFront()
        lastStreamSize?.let { applyWindowedSize(it) }
    }

    fun present(frame: Frame) {
        val target = window ?: return
        if (!target.isVisible) {
            return
        }
        runCatching {
            if (backendLabel == "Canvas2D") {
                val bufferedImage = convertFrame(frame) ?: return@runCatching
                target.showImage(bufferedImage)
            } else {
                target.showImage(frame, false)
            }
        }
    }

    fun updateSnapshot(snapshot: ReceiverSnapshot) {
        window?.title = buildTitle(snapshot)
    }

    fun updateStreamSize(size: Dimension?) {
        if (size == null) {
            return
        }
        lastStreamSize = Dimension(size)
        if (!fullscreen) {
            applyWindowedSize(size)
        }
    }

    fun toggleFullscreen() {
        if (window == null) {
            openOrFocus()
        }

        if (fullscreen) {
            exitFullscreen()
        } else {
            enterFullscreen()
        }
    }

    fun hide() {
        val target = window ?: return
        if (fullscreen) {
            exitFullscreen()
        }
        target.isVisible = false
    }

    fun close() {
        val target = window ?: return
        if (fullscreen) {
            exitFullscreen()
        }
        runCatching { target.dispose() }
        runCatching { converter.close() }
        window = null
        backendLabel = "Closed"
    }

    private fun createWindow(): CanvasFrame {
        return try {
            backendLabel = "OpenGL"
            GLCanvasFrame("Everty Game View")
        } catch (_: Throwable) {
            backendLabel = "Canvas2D"
            CanvasFrame("Everty Game View")
        }.apply {
            defaultCloseOperation = JFrame.HIDE_ON_CLOSE
            setLatency(0)
            background = Color.BLACK
            contentPane.background = Color.BLACK
        }
    }

    private fun configureWindow(created: CanvasFrame) {
        created.getCanvas().background = Color.BLACK
        created.addWindowListener(object : java.awt.event.WindowAdapter() {
            override fun windowClosing(e: java.awt.event.WindowEvent) {
                if (fullscreen) {
                    exitFullscreen()
                }
                created.isVisible = false
            }
        })
        created.getCanvas().addMouseListener(object : MouseAdapter() {
            override fun mouseClicked(e: MouseEvent) {
                if (e.button == MouseEvent.BUTTON1 && e.clickCount == 2) {
                    toggleFullscreen()
                }
            }
        })
        bindHotkeys(created)
    }

    private fun bindHotkeys(target: CanvasFrame) {
        val inputMap = target.rootPane.getInputMap(JComponent.WHEN_IN_FOCUSED_WINDOW)
        val actionMap = target.rootPane.actionMap

        fun register(actionKey: String, keyStroke: KeyStroke, action: () -> Unit) {
            inputMap.put(keyStroke, actionKey)
            actionMap.put(actionKey, object : AbstractAction() {
                override fun actionPerformed(e: java.awt.event.ActionEvent?) {
                    action()
                }
            })
        }

        register("toggle-game-fullscreen-f11", KeyStroke.getKeyStroke(KeyEvent.VK_F11, 0)) {
            toggleFullscreen()
        }
        register("toggle-game-fullscreen-alt-enter", KeyStroke.getKeyStroke(KeyEvent.VK_ENTER, InputEvent.ALT_DOWN_MASK)) {
            toggleFullscreen()
        }
        register("hide-game-view", KeyStroke.getKeyStroke(KeyEvent.VK_ESCAPE, 0)) {
            if (fullscreen) {
                exitFullscreen()
            } else {
                hide()
            }
        }
    }

    private fun enterFullscreen() {
        val target = window ?: return
        if (fullscreen) {
            return
        }

        val device = target.graphicsConfiguration.device
        val deviceBounds = device.defaultConfiguration.bounds
        restoreBounds = target.bounds
        restoreExtendedState = target.extendedState
        fullscreen = true
        fullscreenDevice = device
        target.dispose()
        target.isUndecorated = true
        target.isResizable = false
        target.extendedState = JFrame.NORMAL
        target.isAlwaysOnTop = true
        exclusiveFullscreen = false
        if (device.isFullScreenSupported) {
            runCatching {
                device.fullScreenWindow = target
                exclusiveFullscreen = device.fullScreenWindow === target
            }
        }
        if (!exclusiveFullscreen) {
            target.bounds = deviceBounds
            target.isVisible = true
        }
        runCatching { target.setCanvasSize(deviceBounds.width, deviceBounds.height) }
        target.toFront()
        target.requestFocus()
    }

    private fun exitFullscreen() {
        val target = window ?: return
        if (!fullscreen) {
            return
        }

        fullscreen = false
        runCatching { fullscreenDevice?.fullScreenWindow = null }
        fullscreenDevice = null
        exclusiveFullscreen = false
        target.dispose()
        target.isUndecorated = false
        target.isResizable = true
        val previousBounds = restoreBounds
        if (previousBounds != null) {
            target.bounds = previousBounds
        }
        target.extendedState = restoreExtendedState
        target.isAlwaysOnTop = false
        target.isVisible = true
        lastStreamSize?.let { applyWindowedSize(it) }
        target.toFront()
        target.requestFocus()
    }

    private fun applyWindowedSize(streamSize: Dimension) {
        val target = window ?: return
        if (fullscreen) {
            return
        }

        val screenBounds = GraphicsEnvironment.getLocalGraphicsEnvironment().maximumWindowBounds
        val maxWidth = (screenBounds.width * 0.88).toInt().coerceAtLeast(640)
        val maxHeight = (screenBounds.height * 0.88).toInt().coerceAtLeast(360)
        val scale = minOf(
            maxWidth.toDouble() / streamSize.width,
            maxHeight.toDouble() / streamSize.height,
        )

        val targetWidth = (streamSize.width * scale).toInt().coerceAtLeast(480)
        val targetHeight = (streamSize.height * scale).toInt().coerceAtLeast(270)
        target.setCanvasSize(targetWidth, targetHeight)
        target.pack()
        target.setLocationRelativeTo(null)
    }

    private fun buildTitle(snapshot: ReceiverSnapshot): String {
        return buildString {
            append("Everty Game View")
            append(" [")
            append(backendLabel)
            append("]")
            if (snapshot.sessionCodec != "-") {
                append("  ")
                append(snapshot.sessionCodec)
            }
            if (snapshot.resolution != "-") {
                append("  ")
                append(snapshot.resolution)
            }
            if (snapshot.decodePath != "-") {
                append("  decode:")
                append(snapshot.decodePath)
            }
            if (snapshot.decodeFps > 0) {
                append("  ")
                append(snapshot.decodeFps)
                append(" fps")
            }
        }
    }

    private fun convertFrame(frame: Frame): BufferedImage? {
        val frameImage = frame.image
        if (frameImage == null || frameImage.isEmpty()) {
            return null
        }
        return runCatching { converter.convert(frame) }.getOrNull()
    }
}
