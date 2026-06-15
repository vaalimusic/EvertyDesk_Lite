package ru.everty.desklite

import android.content.Context
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.graphics.RectF
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.view.GestureDetector
import android.view.HapticFeedbackConstants
import android.view.MotionEvent
import android.view.View
import android.view.ViewConfiguration
import kotlin.math.abs
import kotlin.math.hypot
import kotlin.math.roundToInt

/**
 * Тачпад без картинки (слепой режим):
 *  • 1 палец move          → курсор (MouseMove)
 *  • 1 палец tap           → левый клик (click feedback)
 *  • 1 палец long press    → drag-select (зажать ЛКМ + двигать для выделения текста)
 *  • 2 пальца tap < 500ms  → правый клик (Mac-style, click feedback)
 *  • 2 пальца drag         → вертикальный скролл (с axis-lock)
 *
 * Shake-to-find: быстрое движение пальцем → курсор временно увеличивается (как macOS).
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
        color = Color.rgb(0xFF, 0x99, 0x33)
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
    private var dragSelectMode = false
    private var sensitivity = 1.35f

    // Sub-pixel accumulator — prevents jitter from roundToInt discarding fractions
    private var moveAccumX = 0f
    private var moveAccumY = 0f

    // ── Shake-to-find cursor (like macOS) ─────────────────────────────────────
    // Fast movement temporarily enlarges the cursor so it's easy to locate.
    private var cursorScale = 1f
    private var lastMoveMs = 0L
    private val resetCursorScaleRunnable = Runnable { cursorScale = 1f; invalidate() }

    // ── 2-пальца ─────────────────────────────────────────────────────────────
    private var twoFinger = false
    private var prevMidX = 0f
    private var prevMidY = 0f
    private var twoFingerDownTime = 0L
    private var twoFingerEndTime = 0L
    private var twoFingerDownMidX = 0f
    private var twoFingerDownMidY = 0f
    private var twoFingerMoved = false

    // Scroll accumulators + axis-lock (prevents diagonal drift)
    private var scrollAccumX = 0f
    private var scrollAccumY = 0f
    private val scrollStepPx = 28f * density
    private var scrollAxisLocked = false
    private var scrollIsVertical = true
    private val AXIS_LOCK_THRESHOLD = 14f * density  // lock axis after 14dp total movement

    // ── Scroll direction ──────────────────────────────────────────────────────
    // naturalScroll=true  (Mac default): swipe down → content follows finger → page scrolls down
    // naturalScroll=false (traditional): swipe down → page scrolls up (like a scroll wheel)
    private var naturalScroll = true

    companion object {
        private const val TWO_FINGER_TAP_MS = 500L
        private const val TWO_FINGER_TAP_SLOP = 80f
        private const val TWO_FINGER_COOLDOWN = 500L
        private const val SHAKE_SPEED_PX_PER_SEC = 2200f  // touch px/s threshold
        private const val SHAKE_SCALE = 3.5f
        private const val SHAKE_HOLD_MS = 700L
    }

    // ── Click feedback ────────────────────────────────────────────────────────
    private var clickFeedback = 0f
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
                longPressFired = true
                dragSelectMode = true
                client.touch(cursorX, cursorY, 0)
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

    fun setNaturalScroll(enabled: Boolean) {
        naturalScroll = enabled
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
            val progress = 1f - clickFeedback
            val radius = (14f + progress * 32f) * density
            val alpha = (clickFeedback * 220).toInt()
            val base = if (clickIsRight) Color.rgb(0xCC, 0x55, 0x22) else Color.rgb(0x12, 0xC9, 0x72)
            clickRingPaint.color = Color.argb(alpha, Color.red(base), Color.green(base), Color.blue(base))
            canvas.drawCircle(vcx, vcy, radius, clickRingPaint)
        }

        // Курсор — размер растёт при быстром движении (shake-to-find)
        val baseR = 11f * density * cursorScale
        val strokeR = 16f * density * cursorScale
        val fill = if (dragSelectMode) cursorDragPaint else cursorNormalPaint
        canvas.drawCircle(vcx, vcy, baseR, fill)
        canvas.drawCircle(vcx, vcy, strokeR, cursorStrokePaint)

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
                moveAccumX = 0f; moveAccumY = 0f
                lastMoveMs = SystemClock.elapsedRealtime()
            }

            MotionEvent.ACTION_POINTER_DOWN -> {
                if (event.pointerCount == 2) {
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
                    scrollAccumX = 0f; scrollAccumY = 0f
                    scrollAxisLocked = false
                    scrollIsVertical = true
                }
            }

            MotionEvent.ACTION_MOVE -> {
                if (twoFinger && event.pointerCount >= 2) {
                    handleTwoFingerMove(event)
                } else if (!twoFinger) {
                    handleOneFingerMove(event)
                }
            }

            MotionEvent.ACTION_POINTER_UP -> {
                if (twoFinger && event.pointerCount == 2) {
                    twoFingerEndTime = System.currentTimeMillis()
                    val elapsed = twoFingerEndTime - twoFingerDownTime

                    if (!twoFingerMoved && elapsed < TWO_FINGER_TAP_MS) {
                        client.rightClick(cursorX, cursorY)
                        triggerClick(true)
                        longPressFired = true
                    }

                    val ri = if (event.actionIndex == 0) 1 else 0
                    prevX = event.getX(ri); prevY = event.getY(ri)
                    downX = prevX; downY = prevY
                    moveAccumX = 0f; moveAccumY = 0f
                    lastMoveMs = SystemClock.elapsedRealtime()
                    twoFinger = false
                    scrollAccumX = 0f; scrollAccumY = 0f
                    invalidate()
                }
            }

            MotionEvent.ACTION_UP -> {
                when {
                    dragSelectMode -> {
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
                scrollAccumX = 0f; scrollAccumY = 0f
                invalidate()
            }
        }
        return true
    }

    // ── 1-палец: движение с sub-pixel аккумулятором и shake-to-find ──────────

    private fun handleOneFingerMove(event: MotionEvent) {
        val rawDx = event.x - prevX
        val rawDy = event.y - prevY

        // Velocity for shake-to-find
        val now = SystemClock.elapsedRealtime()
        val dtMs = (now - lastMoveMs).coerceAtLeast(1).toFloat()
        val speed = hypot(rawDx, rawDy) / dtMs * 1000f  // touch px/s
        lastMoveMs = now
        if (speed > SHAKE_SPEED_PX_PER_SEC) {
            cursorScale = SHAKE_SCALE
            handler.removeCallbacks(resetCursorScaleRunnable)
            handler.postDelayed(resetCursorScaleRunnable, SHAKE_HOLD_MS)
        }

        // Sub-pixel accumulator — avoids jitter from rounding discarding fractions
        moveAccumX += rawDx * sensitivity
        moveAccumY += rawDy * sensitivity
        prevX = event.x; prevY = event.y

        val dx = moveAccumX.toInt()
        val dy = moveAccumY.toInt()
        moveAccumX -= dx
        moveAccumY -= dy

        if (dx != 0 || dy != 0) {
            cursorX = clampX(cursorX + dx)
            cursorY = clampY(cursorY + dy)
            client.touch(cursorX, cursorY, 1)
            invalidate()
        }
    }

    // ── 2-пальца: скролл с axis-lock (нет диагонального дрейфа) ─────────────

    private fun handleTwoFingerMove(event: MotionEvent) {
        val midX = (event.getX(0) + event.getX(1)) / 2f
        val midY = (event.getY(0) + event.getY(1)) / 2f
        val dx = midX - prevMidX
        val dy = midY - prevMidY
        prevMidX = midX; prevMidY = midY

        val drift = hypot(midX - twoFingerDownMidX, midY - twoFingerDownMidY)
        if (drift > TWO_FINGER_TAP_SLOP) twoFingerMoved = true

        scrollAccumX += dx
        scrollAccumY += dy

        // Lock scroll axis after enough movement to determine intent
        if (!scrollAxisLocked) {
            val totalMovement = abs(scrollAccumX) + abs(scrollAccumY)
            if (totalMovement > AXIS_LOCK_THRESHOLD) {
                scrollIsVertical = abs(scrollAccumY) >= abs(scrollAccumX)
                scrollAxisLocked = true
                // Discard off-axis accumulation
                if (scrollIsVertical) scrollAccumX = 0f else scrollAccumY = 0f
            }
        }

        if (scrollAxisLocked && scrollIsVertical) {
            val steps = (scrollAccumY / scrollStepPx).toInt()
            if (steps != 0) {
                scrollAccumY -= steps * scrollStepPx
                val delta = if (naturalScroll) -steps else steps
                client.scroll(cursorX, cursorY, delta)
            }
        }

        // Always redraw so cursor stays visible during scroll
        invalidate()
    }

    private fun clampX(v: Int) = v.coerceIn(remoteLeft, remoteLeft + remoteW - 1)
    private fun clampY(v: Int) = v.coerceIn(remoteTop, remoteTop + remoteH - 1)
}
