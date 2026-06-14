package ru.everty.desklite

import android.content.Context
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.graphics.RectF
import android.os.Handler
import android.os.Looper
import android.view.GestureDetector
import android.view.HapticFeedbackConstants
import android.view.MotionEvent
import android.view.View
import android.view.ViewConfiguration
import kotlin.math.hypot
import kotlin.math.roundToInt

/**
 * Тачпад без картинки (слепой режим):
 *  • 1 палец move          → курсор (MouseMove)
 *  • 1 палец tap           → левый клик (click feedback)
 *  • 1 палец long press    → drag-select (зажать ЛКМ + двигать для выделения текста)
 *  • 2 пальца tap < 500ms  → правый клик (Mac-style, click feedback)
 *  • 2 пальца drag         → вертикальный скролл
 */
class TouchpadView(context: Context, private val client: NativeClient) : View(context) {

    private val density = resources.displayMetrics.density
    private val tapSlop = ViewConfiguration.get(context).scaledTouchSlop.toFloat()
    private val handler = Handler(Looper.getMainLooper())

    // ── Paints ────────────────────────────────────────────────────────────────
    private val cursorNormalPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = Color.rgb(18, 201, 114)
    }
    private val cursorDragPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = Color.rgb(0xFF, 0x99, 0x33)  // оранжевый в drag-режиме
    }
    private val cursorStrokePaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = Color.WHITE
        strokeWidth = 2.5f * density
        style = Paint.Style.STROKE
    }
    private val clickRingPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        style = Paint.Style.STROKE
        strokeWidth = 3f * density
    }
    private val panelPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = Color.rgb(14, 18, 16)
    }
    private val linePaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = Color.rgb(32, 46, 39)
        strokeWidth = 1f * density
    }
    private val titlePaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = Color.WHITE
        textSize = 20f * density
        isFakeBoldText = true
    }
    private val hintPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = Color.rgb(210, 222, 216)
        textSize = 13f * density
    }
    private val coordPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = Color.rgb(120, 140, 130)
        textSize = 12f * density
    }
    private val dragBadgeBgPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = Color.argb(0xCC, 0xFF, 0x88, 0x22)
    }
    private val dragBadgeTextPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = Color.WHITE
        textSize = 11f * density
        isFakeBoldText = true
    }

    // ── Remote geometry ───────────────────────────────────────────────────────
    private var remoteLeft = 0
    private var remoteTop = 0
    private var remoteW = 1920
    private var remoteH = 1080
    private var cursorX = remoteW / 2
    private var cursorY = remoteH / 2

    // ── 1-палец ───────────────────────────────────────────────────────────────
    private var prevX = 0f
    private var prevY = 0f
    private var downX = 0f
    private var downY = 0f
    private var longPressFired = false
    private var dragSelectMode = false   // ЛКМ зажата, движение = выделение текста
    private var sensitivity = 1.35f

    // ── 2-пальца ─────────────────────────────────────────────────────────────
    private var twoFinger = false
    private var prevMidX = 0f
    private var prevMidY = 0f
    private var twoFingerDownTime = 0L
    private var twoFingerEndTime = 0L
    private var twoFingerDownMidX = 0f
    private var twoFingerDownMidY = 0f
    private var twoFingerMoved = false
    private var scrollAccumY = 0f
    private val scrollStepPx = 28f * density

    companion object {
        private const val TWO_FINGER_TAP_MS = 500L
        private const val TWO_FINGER_TAP_SLOP = 80f    // px; дрейф при тапе на телефоне
        private const val TWO_FINGER_COOLDOWN = 500L   // cooldown long press после 2-пальц. жеста
    }

    // ── Click feedback ────────────────────────────────────────────────────────
    private var clickFeedback = 0f   // 1.0 = только нажали, плавно до 0
    private var clickIsRight = false

    private val clickFadeRunnable = object : Runnable {
        override fun run() {
            if (clickFeedback > 0f) {
                clickFeedback = (clickFeedback - 0.1f).coerceAtLeast(0f)
                invalidate()
                if (clickFeedback > 0f) handler.postDelayed(this, 16)
            }
        }
    }

    private fun triggerClick(isRight: Boolean) {
        clickIsRight = isRight
        clickFeedback = 1f
        handler.removeCallbacks(clickFadeRunnable)
        handler.post(clickFadeRunnable)
        performHapticFeedback(
            if (isRight) HapticFeedbackConstants.LONG_PRESS
            else HapticFeedbackConstants.VIRTUAL_KEY
        )
    }

    // ── GestureDetector — только long press ───────────────────────────────────
    private val gestureDetector = GestureDetector(context,
        object : GestureDetector.SimpleOnGestureListener() {
            override fun onLongPress(e: MotionEvent) {
                if (twoFinger) return
                if (System.currentTimeMillis() - twoFingerEndTime < TWO_FINGER_COOLDOWN) return
                // Long press = drag-select: зажимаем ЛКМ, двигаем — выделяем текст
                longPressFired = true
                dragSelectMode = true
                client.touch(cursorX, cursorY, 0)  // mouse DOWN
                triggerClick(false)
                invalidate()
            }
        }
    )

    // ── Public API ────────────────────────────────────────────────────────────

    fun refreshRemoteSize() {
        client.remoteGeometry()?.let { g ->
            if (g.width > 0 && g.height > 0 &&
                (g.x != remoteLeft || g.y != remoteTop || g.width != remoteW || g.height != remoteH)
            ) {
                remoteLeft = g.x; remoteTop = g.y
                remoteW = g.width; remoteH = g.height
                cursorX = clampX(cursorX); cursorY = clampY(cursorY)
                invalidate()
            }
        }
    }

    fun centerCursor() {
        refreshRemoteSize()
        cursorX = remoteLeft + remoteW / 2
        cursorY = remoteTop + remoteH / 2
        client.touch(cursorX, cursorY, 1)
        invalidate()
    }

    fun rightClickAtCursor() {
        client.rightClick(cursorX, cursorY)
        triggerClick(true)
        invalidate()
    }

    fun setSensitivity(value: Float) {
        sensitivity = value.coerceIn(0.6f, 2.4f)
    }

    // ── Draw ──────────────────────────────────────────────────────────────────

    override fun onDraw(canvas: Canvas) {
        canvas.drawColor(Color.rgb(6, 8, 7))

        val pad = 18f * density
        val panel = RectF(pad, pad, width - pad, height - pad)
        canvas.drawRoundRect(panel, 22f * density, 22f * density, panelPaint)
        canvas.drawLine(
            panel.left, panel.top + 62f * density,
            panel.right, panel.top + 62f * density,
            linePaint,
        )

        canvas.drawText(
            if (dragSelectMode) "EvertyDesk — ВЫДЕЛЕНИЕ ТЕКСТА" else "EvertyDesk Touchpad",
            panel.left + 18f * density, panel.top + 38f * density, titlePaint,
        )

        val originText = if (remoteLeft != 0 || remoteTop != 0) " @ $remoteLeft,$remoteTop" else ""
        canvas.drawText(
            "Desktop ${remoteW}×${remoteH}$originText  •  tap=клик  •  2 пальца тап=ПКМ  •  hold=выделение",
            panel.left + 18f * density, panel.top + 88f * density, hintPaint,
        )

        // Позиция курсора в координатах view
        val vcx = panel.left + ((cursorX - remoteLeft).toFloat() / remoteW.coerceAtLeast(1)) * panel.width()
        val vcy = panel.top + ((cursorY - remoteTop).toFloat() / remoteH.coerceAtLeast(1)) * panel.height()

        // Click feedback: расширяющееся кольцо с затуханием
        if (clickFeedback > 0f) {
            val progress = 1f - clickFeedback          // 0=только нажали → 1=угасает
            val radius = (14f + progress * 32f) * density
            val alpha = (clickFeedback * 220).toInt()
            val base = if (clickIsRight) Color.rgb(0xCC, 0x55, 0x22) else Color.rgb(0x12, 0xC9, 0x72)
            clickRingPaint.color = Color.argb(alpha, Color.red(base), Color.green(base), Color.blue(base))
            canvas.drawCircle(vcx, vcy, radius, clickRingPaint)
        }

        // Курсор: зелёный обычно, оранжевый в drag-режиме
        val fill = if (dragSelectMode) cursorDragPaint else cursorNormalPaint
        canvas.drawCircle(vcx, vcy, 11f * density, fill)
        canvas.drawCircle(vcx, vcy, 16f * density, cursorStrokePaint)

        // Бейдж "DRAG" над курсором
        if (dragSelectMode) {
            val label = "DRAG"
            val tw = dragBadgeTextPaint.measureText(label)
            val bp = 5f * density
            val bx = vcx - tw / 2f
            val by = vcy - 24f * density
            canvas.drawRoundRect(
                RectF(bx - bp, by - 13f * density, bx + tw + bp, by + 3f * density),
                6f * density, 6f * density, dragBadgeBgPaint,
            )
            canvas.drawText(label, bx, by, dragBadgeTextPaint)
        }

        canvas.drawText(
            "x=$cursorX  y=$cursorY",
            panel.left + 18f * density, panel.bottom - 22f * density, coordPaint,
        )
    }

    // ── Touch ─────────────────────────────────────────────────────────────────

    override fun onTouchEvent(event: MotionEvent): Boolean {
        refreshRemoteSize()
        gestureDetector.onTouchEvent(event)

        when (event.actionMasked) {

            MotionEvent.ACTION_DOWN -> {
                longPressFired = false
                dragSelectMode = false
                twoFinger = false
                downX = event.x; downY = event.y
                prevX = event.x; prevY = event.y
            }

            MotionEvent.ACTION_POINTER_DOWN -> {
                if (event.pointerCount == 2) {
                    // Если были в drag-режиме — отпустить кнопку
                    if (dragSelectMode) {
                        client.touch(cursorX, cursorY, 2)
                        dragSelectMode = false
                    }
                    twoFinger = true
                    longPressFired = false
                    val midX = (event.getX(0) + event.getX(1)) / 2f
                    val midY = (event.getY(0) + event.getY(1)) / 2f
                    prevMidX = midX; prevMidY = midY
                    twoFingerDownTime = System.currentTimeMillis()
                    twoFingerDownMidX = midX; twoFingerDownMidY = midY
                    twoFingerMoved = false
                    scrollAccumY = 0f
                }
            }

            MotionEvent.ACTION_MOVE -> {
                if (twoFinger && event.pointerCount >= 2) {
                    val midX = (event.getX(0) + event.getX(1)) / 2f
                    val midY = (event.getY(0) + event.getY(1)) / 2f
                    val dy = midY - prevMidY
                    prevMidX = midX; prevMidY = midY

                    // Проверяем смещение от начала жеста
                    val drift = hypot(midX - twoFingerDownMidX, midY - twoFingerDownMidY)
                    if (drift > TWO_FINGER_TAP_SLOP) twoFingerMoved = true

                    // Скролл с аккумулятором
                    scrollAccumY += dy
                    val steps = (scrollAccumY / scrollStepPx).toInt()
                    if (steps != 0) {
                        scrollAccumY -= steps * scrollStepPx
                        client.scroll(cursorX, cursorY, -steps)
                    }
                } else if (!twoFinger) {
                    val dx = ((event.x - prevX) * sensitivity).roundToInt()
                    val dy = ((event.y - prevY) * sensitivity).roundToInt()
                    prevX = event.x; prevY = event.y
                    if (dx != 0 || dy != 0) {
                        cursorX = clampX(cursorX + dx)
                        cursorY = clampY(cursorY + dy)
                        // В drag-select: action=1 (move) — хост помнит зажатую кнопку
                        client.touch(cursorX, cursorY, 1)
                        invalidate()
                    }
                }
            }

            MotionEvent.ACTION_POINTER_UP -> {
                if (twoFinger && event.pointerCount == 2) {
                    twoFingerEndTime = System.currentTimeMillis()
                    val elapsed = twoFingerEndTime - twoFingerDownTime

                    if (!twoFingerMoved && elapsed < TWO_FINGER_TAP_MS) {
                        // 2-пальцевый тап → правый клик
                        client.rightClick(cursorX, cursorY)
                        triggerClick(true)
                        longPressFired = true  // не пустить левый клик в ACTION_UP
                    }

                    // Реинициализируем 1-пальц. трекинг с оставшегося пальца
                    val ri = if (event.actionIndex == 0) 1 else 0
                    prevX = event.getX(ri); prevY = event.getY(ri)
                    downX = prevX; downY = prevY
                    twoFinger = false
                    scrollAccumY = 0f
                    invalidate()
                }
            }

            MotionEvent.ACTION_UP -> {
                when {
                    dragSelectMode -> {
                        // Отпускаем зажатую ЛКМ — текст выделен
                        client.touch(cursorX, cursorY, 2)
                        dragSelectMode = false
                    }
                    !twoFinger && !longPressFired -> {
                        val dist = hypot(event.x - downX, event.y - downY)
                        if (dist < tapSlop) {
                            client.touch(cursorX, cursorY, 0)
                            client.touch(cursorX, cursorY, 2)
                            triggerClick(false)
                        }
                    }
                }
                twoFinger = false
                longPressFired = false
                invalidate()
            }

            MotionEvent.ACTION_CANCEL -> {
                if (dragSelectMode) {
                    client.touch(cursorX, cursorY, 2)
                    dragSelectMode = false
                }
                twoFinger = false
                longPressFired = false
                twoFingerEndTime = System.currentTimeMillis()
                scrollAccumY = 0f
                invalidate()
            }
        }
        return true
    }

    private fun clampX(v: Int) = v.coerceIn(remoteLeft, remoteLeft + remoteW - 1)
    private fun clampY(v: Int) = v.coerceIn(remoteTop, remoteTop + remoteH - 1)
}
