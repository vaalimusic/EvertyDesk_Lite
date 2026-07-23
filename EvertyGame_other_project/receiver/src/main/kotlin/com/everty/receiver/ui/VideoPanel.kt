package com.everty.receiver.ui

import java.io.Closeable
import java.awt.Color
import java.awt.Dimension
import java.awt.Font
import java.awt.Graphics
import java.awt.Graphics2D
import java.awt.RenderingHints
import java.awt.image.BufferedImage
import org.bytedeco.javacv.Frame
import org.bytedeco.javacv.Java2DFrameConverter
import javax.swing.JPanel

class VideoPanel : JPanel(), Closeable {
    private val converter = Java2DFrameConverter()
    @Volatile
    private var image: BufferedImage? = null
    @Volatile
    private var displayScaleMode: DisplayScaleMode = DisplayScaleMode.FIT
    @Volatile
    private var overlayVisible: Boolean = true
    @Volatile
    private var overlayLines: List<String> = emptyList()

    init {
        background = Color.BLACK
        preferredSize = Dimension(960, 540)
        isDoubleBuffered = false
    }

    fun updateFrame(frame: Frame) {
        val frameImage = frame.image
        if (frameImage == null || frameImage.isEmpty()) {
            return
        }
        image = runCatching { converter.convert(frame) }.getOrNull() ?: return
        repaint()
    }

    fun setDisplayScaleMode(mode: DisplayScaleMode) {
        displayScaleMode = mode
        repaint()
    }

    fun setOverlayVisible(visible: Boolean) {
        overlayVisible = visible
        repaint()
    }

    fun setOverlayLines(lines: List<String>) {
        overlayLines = lines
        repaint()
    }

    override fun paintComponent(graphics: Graphics) {
        super.paintComponent(graphics)
        val g2 = graphics as Graphics2D
        g2.setRenderingHint(RenderingHints.KEY_INTERPOLATION, RenderingHints.VALUE_INTERPOLATION_NEAREST_NEIGHBOR)
        g2.setRenderingHint(RenderingHints.KEY_RENDERING, RenderingHints.VALUE_RENDER_SPEED)
        g2.setRenderingHint(RenderingHints.KEY_COLOR_RENDERING, RenderingHints.VALUE_COLOR_RENDER_SPEED)
        g2.color = background
        g2.fillRect(0, 0, width, height)

        val imageToDraw = image
        if (imageToDraw != null) {
            val targetBounds = when (displayScaleMode) {
                DisplayScaleMode.FIT -> {
                    val scale = minOf(width.toDouble() / imageToDraw.width, height.toDouble() / imageToDraw.height)
                    scaledBounds(imageToDraw, scale)
                }

                DisplayScaleMode.FILL -> {
                    val scale = maxOf(width.toDouble() / imageToDraw.width, height.toDouble() / imageToDraw.height)
                    scaledBounds(imageToDraw, scale)
                }

                DisplayScaleMode.STRETCH -> DrawBounds(
                    x = 0,
                    y = 0,
                    width = width,
                    height = height,
                )
            }

            g2.drawImage(
                imageToDraw,
                targetBounds.x,
                targetBounds.y,
                targetBounds.width,
                targetBounds.height,
                null,
            )
        }

        if (overlayVisible && overlayLines.isNotEmpty()) {
            paintOverlay(g2)
        }
    }

    private fun scaledBounds(imageToDraw: BufferedImage, scale: Double): DrawBounds {
        val targetWidth = (imageToDraw.width * scale).toInt()
        val targetHeight = (imageToDraw.height * scale).toInt()
        return DrawBounds(
            x = (width - targetWidth) / 2,
            y = (height - targetHeight) / 2,
            width = targetWidth,
            height = targetHeight,
        )
    }

    private fun paintOverlay(g2: Graphics2D) {
        val lines = overlayLines
        if (lines.isEmpty()) {
            return
        }

        val overlayFont = Font(Font.MONOSPACED, Font.PLAIN, 13)
        g2.font = overlayFont
        g2.setRenderingHint(RenderingHints.KEY_TEXT_ANTIALIASING, RenderingHints.VALUE_TEXT_ANTIALIAS_ON)
        val metrics = g2.fontMetrics
        val lineHeight = metrics.height
        val paddingX = 12
        val paddingY = 10
        val blockWidth = lines.maxOf { metrics.stringWidth(it) } + paddingX * 2
        val blockHeight = lineHeight * lines.size + paddingY * 2

        g2.color = Color(0, 0, 0, 168)
        g2.fillRoundRect(16, 16, blockWidth, blockHeight, 16, 16)
        g2.color = Color.WHITE
        lines.forEachIndexed { index, line ->
            val y = 16 + paddingY + metrics.ascent + index * lineHeight
            g2.drawString(line, 16 + paddingX, y)
        }
    }

    private data class DrawBounds(
        val x: Int,
        val y: Int,
        val width: Int,
        val height: Int,
    )

    override fun close() {
        runCatching { converter.close() }
    }
}
