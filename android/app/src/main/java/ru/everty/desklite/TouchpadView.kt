package ru.everty.desklite

import android.content.Context
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.graphics.RectF
import android.view.GestureDetector
import android.view.HapticFeedbackConstants
import android.view.MotionEvent
import android.view.View
import android.view.ViewConfiguration
import kotlin.math.hypot
import kotlin.math.roundToInt

class TouchpadView(context: Context, private val client: NativeClient) : View(context) {
    private val density = resources.displayMetrics.density
    private val tapSlop = ViewConfiguration.get(context).scaledTouchSlop.toFloat()
    private val cursorPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply { color = Color.rgb(18, 201, 114) }
    private val cursorStroke = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = Color.WHITE
        strokeWidth = 2.5f * density
        style = Paint.Style.STROKE
    }
    private val textPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = Color.rgb(210, 222, 216)
        textSize = 14f * density
    }
    private val titlePaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = Color.WHITE
        textSize = 20f * density
        isFakeBoldText = true
    }
    private val panelPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = Color.rgb(14, 18, 16)
    }
    private val linePaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = Color.rgb(32, 46, 39)
        strokeWidth = 1f * density
    }

    private var remoteW = 1920
    private var remoteH = 1080
    private var cursorX = remoteW / 2
    private var cursorY = remoteH / 2
    private var prevX = 0f
    private var prevY = 0f
    private var downX = 0f
    private var downY = 0f
    private var twoFinger = false
    private var prevMidY = 0f
    private var longPressFired = false
    private var sensitivity = 1.35f

    private val gestureDetector = GestureDetector(
        context,
        object : GestureDetector.SimpleOnGestureListener() {
            override fun onLongPress(e: MotionEvent) {
                if (twoFinger) return
                longPressFired = true
                performHapticFeedback(HapticFeedbackConstants.LONG_PRESS)
                client.rightClick(cursorX, cursorY)
                invalidate()
            }
        },
    )

    fun refreshRemoteSize() {
        client.remoteSize()?.let { (w, h) ->
            if (w > 0 && h > 0 && (w != remoteW || h != remoteH)) {
                remoteW = w
                remoteH = h
                cursorX = cursorX.coerceIn(0, remoteW - 1)
                cursorY = cursorY.coerceIn(0, remoteH - 1)
                invalidate()
            }
        }
    }

    fun centerCursor() {
        refreshRemoteSize()
        cursorX = remoteW / 2
        cursorY = remoteH / 2
        client.touch(cursorX, cursorY, 1)
        invalidate()
    }

    fun rightClickAtCursor() {
        client.rightClick(cursorX, cursorY)
        performHapticFeedback(HapticFeedbackConstants.VIRTUAL_KEY)
        invalidate()
    }

    fun setSensitivity(value: Float) {
        sensitivity = value.coerceIn(0.6f, 2.4f)
    }

    override fun onDraw(canvas: Canvas) {
        canvas.drawColor(Color.rgb(6, 8, 7))
        val pad = 18f * density
        val panel = RectF(pad, pad, width - pad, height - pad)
        canvas.drawRoundRect(panel, 22f * density, 22f * density, panelPaint)
        canvas.drawLine(panel.left, panel.top + 62f * density, panel.right, panel.top + 62f * density, linePaint)

        canvas.drawText("EvertyDesk Touchpad", panel.left + 18f * density, panel.top + 38f * density, titlePaint)
        canvas.drawText(
            "Display ${remoteW}x$remoteH  |  one finger: cursor  |  tap: click  |  two fingers: scroll",
            panel.left + 18f * density,
            panel.top + 88f * density,
            textPaint,
        )

        val viewCursorX = panel.left + (cursorX.toFloat() / remoteW.coerceAtLeast(1)) * panel.width()
        val viewCursorY = panel.top + (cursorY.toFloat() / remoteH.coerceAtLeast(1)) * panel.height()
        canvas.drawCircle(viewCursorX, viewCursorY, 11f * density, cursorPaint)
        canvas.drawCircle(viewCursorX, viewCursorY, 16f * density, cursorStroke)
        canvas.drawText(
            "x=$cursorX y=$cursorY",
            panel.left + 18f * density,
            panel.bottom - 22f * density,
            textPaint,
        )
    }

    override fun onTouchEvent(event: MotionEvent): Boolean {
        refreshRemoteSize()
        gestureDetector.onTouchEvent(event)

        when (event.actionMasked) {
            MotionEvent.ACTION_POINTER_DOWN -> {
                if (event.pointerCount >= 2) {
                    twoFinger = true
                    longPressFired = false
                    prevMidY = midpointY(event)
                }
            }
            MotionEvent.ACTION_POINTER_UP -> {
                if (event.pointerCount <= 2) {
                    twoFinger = false
                }
            }
            MotionEvent.ACTION_DOWN -> {
                longPressFired = false
                twoFinger = false
                downX = event.x
                downY = event.y
                prevX = event.x
                prevY = event.y
            }
            MotionEvent.ACTION_MOVE -> {
                if (twoFinger && event.pointerCount >= 2) {
                    val midY = midpointY(event)
                    val dy = midY - prevMidY
                    prevMidY = midY
                    val steps = (dy / (28f * density)).roundToInt()
                    if (steps != 0) {
                        client.scroll(cursorX, cursorY, -steps)
                    }
                } else {
                    val dx = ((event.x - prevX) * sensitivity).roundToInt()
                    val dy = ((event.y - prevY) * sensitivity).roundToInt()
                    prevX = event.x
                    prevY = event.y
                    if (dx != 0 || dy != 0) {
                        cursorX = (cursorX + dx).coerceIn(0, remoteW - 1)
                        cursorY = (cursorY + dy).coerceIn(0, remoteH - 1)
                        client.touch(cursorX, cursorY, 1)
                        invalidate()
                    }
                }
            }
            MotionEvent.ACTION_UP -> {
                if (!twoFinger && !longPressFired && hypot(event.x - downX, event.y - downY) < tapSlop) {
                    client.touch(cursorX, cursorY, 0)
                    client.touch(cursorX, cursorY, 2)
                    performHapticFeedback(HapticFeedbackConstants.VIRTUAL_KEY)
                }
                twoFinger = false
                longPressFired = false
            }
            MotionEvent.ACTION_CANCEL -> {
                twoFinger = false
                longPressFired = false
            }
        }
        return true
    }

    private fun midpointY(event: MotionEvent): Float =
        if (event.pointerCount >= 2) {
            (event.getY(0) + event.getY(1)) / 2f
        } else {
            event.y
        }
}
