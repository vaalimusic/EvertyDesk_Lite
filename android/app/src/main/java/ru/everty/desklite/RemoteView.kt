package ru.everty.desklite

import android.content.Context
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.graphics.Rect
import android.os.Handler
import android.os.Looper
import android.view.MotionEvent
import android.view.View

/**
 * Отрисовка удалённого экрана: тянет ARGB-кадры из NativeClient, рисует с
 * сохранением пропорций. Тачи пересчитывает в координаты удалённого экрана
 * и шлёт в Rust.
 */
class RemoteView(context: Context, private val client: NativeClient) : View(context) {
    private var bitmap: Bitmap? = null
    private var frameW = 0
    private var frameH = 0
    private var pixels: IntArray = IntArray(0)
    private val paint = Paint(Paint.FILTER_BITMAP_FLAG)
    private val dst = Rect()

    // Прямоугольник, в котором реально нарисован кадр (для пересчёта тача)
    private var drawLeft = 0f
    private var drawTop = 0f
    private var drawScale = 1f

    private val handler = Handler(Looper.getMainLooper())
    private val refresh = object : Runnable {
        override fun run() {
            pullFrame()
            // ~60 fps опрос
            handler.postDelayed(this, 16)
        }
    }

    fun startRendering() {
        handler.post(refresh)
    }

    fun stopRendering() {
        handler.removeCallbacks(refresh)
    }

    private fun pullFrame() {
        val size = client.frameSize() ?: return
        val (w, h) = size
        if (w <= 0 || h <= 0) return
        if (w != frameW || h != frameH || pixels.size < w * h) {
            frameW = w
            frameH = h
            pixels = IntArray(w * h)
            bitmap = Bitmap.createBitmap(w, h, Bitmap.Config.ARGB_8888)
        }
        val got = client.pollFrame(pixels) ?: return
        bitmap?.setPixels(pixels, 0, frameW, 0, 0, frameW, frameH)
        invalidate()
    }

    override fun onDraw(canvas: Canvas) {
        canvas.drawColor(Color.BLACK)
        val bmp = bitmap ?: return
        if (frameW == 0 || frameH == 0) return

        // Вписываем с сохранением пропорций
        val viewW = width.toFloat()
        val viewH = height.toFloat()
        val scale = minOf(viewW / frameW, viewH / frameH)
        val w = frameW * scale
        val h = frameH * scale
        drawLeft = (viewW - w) / 2f
        drawTop = (viewH - h) / 2f
        drawScale = scale
        dst.set(
            drawLeft.toInt(),
            drawTop.toInt(),
            (drawLeft + w).toInt(),
            (drawTop + h).toInt()
        )
        canvas.drawBitmap(bmp, null, dst, paint)
    }

    override fun onTouchEvent(event: MotionEvent): Boolean {
        if (frameW == 0 || drawScale == 0f) return false
        // Экранные координаты view → координаты удалённого экрана
        val rx = ((event.x - drawLeft) / drawScale).toInt().coerceIn(0, frameW - 1)
        val ry = ((event.y - drawTop) / drawScale).toInt().coerceIn(0, frameH - 1)
        val action = when (event.actionMasked) {
            MotionEvent.ACTION_DOWN -> 0
            MotionEvent.ACTION_MOVE -> 1
            MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> 2
            else -> return false
        }
        client.touch(rx, ry, action)
        return true
    }
}
