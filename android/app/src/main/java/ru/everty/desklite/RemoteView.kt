package ru.everty.desklite

import android.content.Context
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Matrix
import android.graphics.Paint
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.util.Log
import android.view.GestureDetector
import android.view.HapticFeedbackConstants
import android.view.MotionEvent
import android.view.ScaleGestureDetector
import android.view.View
import android.view.ViewConfiguration
import kotlin.math.abs
import kotlin.math.hypot
import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToInt

/**
 * Удалённый экран — trackpad-режим (как Mac):
 *  • 1 палец move          → MouseMove (курсор без нажатия)
 *  • 1 палец tap           → левый клик
 *  • 1 палец long press    → правый клик (вибрация; fallback)
 *  • 2 пальца tap (< 500ms, < 80px) → правый клик (Mac-style)
 *  • 2 пальца drag (zoom=1) → вертикальный скролл (с axis-lock)
 *  • 2 пальца drag (zoom>1) → пан картинки
 *  • 2 пальца pinch        → зум (ScaleGestureDetector)
 *
 * Shake-to-find: быстрое движение → курсор временно увеличивается (как macOS).
 */
class RemoteView(context: Context, private val client: NativeClient) : View(context) {

    private val density = resources.displayMetrics.density

    // ── кадр ─────────────────────────────────────────────────────────────────
    private var bitmap: Bitmap? = null
    private var frameW = 0
    private var frameH = 0
    private var pixels = IntArray(0)
    private val paint = Paint(Paint.FILTER_BITMAP_FLAG)

    // ── трансформ (зум + пан) ────────────────────────────────────────────────
    private val matrix = Matrix()
    private val matrixInv = Matrix()

    private var baseFit = 1f
    private var userZoom = 1f
    private var panX = 0f
    private var panY = 0f

    private val tapSlop = ViewConfiguration.get(context).scaledTouchSlop.toFloat()

    // ── виртуальный рабочий стол ────────────────────────────────────────────
    private var geomX = 0
    private var geomY = 0
    private var geomW = 0
    private var geomH = 0

    // ── состояние курсора (трекпад) ──────────────────────────────────────────
    private var lastRemoteX = 0
    private var lastRemoteY = 0
    private var prevFingerX = 0f
    private var prevFingerY = 0f
    private var downViewX = 0f
    private var downViewY = 0f
    private var longPressFired = false
    private var cursorViewX = -1f
    private var cursorViewY = -1f
    private var cursorVisible = false
    private val hideCursorRunnable = Runnable { cursorVisible = false; invalidate() }

    // Sub-pixel accumulator for remote cursor — avoids integer truncation jitter
    private var remoteAccumX = 0f
    private var remoteAccumY = 0f

    // ── Shake-to-find cursor (like macOS) ─────────────────────────────────────
    private var cursorScale = 1f
    private var lastMoveMs = 0L
    private val resetCursorScaleRunnable = Runnable { cursorScale = 1f; invalidate() }

    companion object {
        private const val TWO_FINGER_TAP_MS = 500L
        private const val TWO_FINGER_TAP_SLOP = 80f
        private const val TWO_FINGER_COOLDOWN = 500L
        private const val SCROLL_STEP_PX = 30f
        private const val MIN_SCALE = 0.001f
        private const val AXIS_LOCK_THRESHOLD_DP = 14f  // dp before locking scroll axis
        private const val SHAKE_SPEED_PX_PER_SEC = 2200f
        private const val SHAKE_SCALE = 3.5f
        private const val SHAKE_HOLD_MS = 700L
    }

    // ── двухпальцевый режим ──────────────────────────────────────────────────
    private var twoFingerMode = false
    private var prevMidX = 0f
    private var prevMidY = 0f
    private var twoFingerDownTime = 0L
    private var twoFingerEndTime = 0L
    private var twoFingerDownMidX = 0f
    private var twoFingerDownMidY = 0f
    private var twoFingerMoved = false

    // Scroll state + axis-lock
    private var scrollAccumX = 0f
    private var scrollAccumY = 0f
    private var scrollAxisLocked = false
    private var scrollIsVertical = true

    // ── Scroll direction ──────────────────────────────────────────────────────
    // naturalScroll=true  (Mac default): swipe down → content follows finger
    // naturalScroll=false (traditional): swipe down → page scrolls up
    private var naturalScroll = true

    // ── hardware decode mode (H264/H265/AV1 → TextureView via MediaCodec Surface) ──
    // When true: no RGBA data, TextureView renders video; RemoteView only draws cursor.
    var isHardwareDecodeMode = false
    // When true: all input to host is blocked — only local pinch-zoom is allowed.
    var isGameMode = false
    // Callback fired on every matrix change (zoom, pan, resize) — for TextureView placement.
    data class VideoRect(val x: Int, val y: Int, val width: Int, val height: Int)
    var onVideoRectChanged: ((VideoRect) -> Unit)? = null

    // ── рендер-тик ──────────────────────────────────────────────────────────
    private val handler = Handler(Looper.getMainLooper())
    private val refreshTick = object : Runnable {
        override fun run() { pullFrame(); handler.postDelayed(this, 16) }
    }

    // ── callback для режима «следующий тап = правый клик» ───────────────────
    private var rightClickCallback: ((Int, Int) -> Boolean)? = null
    fun setRightClickCallback(cb: (Int, Int) -> Boolean) { rightClickCallback = cb }

    // ── детекторы жестов ────────────────────────────────────────────────────
    private val gestureDetector = GestureDetector(context,
        object : GestureDetector.SimpleOnGestureListener() {
            override fun onLongPress(e: MotionEvent) {
                if (twoFingerMode) return
                if (System.currentTimeMillis() - twoFingerEndTime < TWO_FINGER_COOLDOWN) return
                longPressFired = true
                performHapticFeedback(HapticFeedbackConstants.LONG_PRESS)
                client.rightClick(lastRemoteX, lastRemoteY)
            }
        }
    )

    private val scaleDetector = ScaleGestureDetector(context,
        object : ScaleGestureDetector.SimpleOnScaleGestureListener() {
            override fun onScale(detector: ScaleGestureDetector): Boolean {
                twoFingerMoved = true  // prevent right-click after pinch
                val factor = detector.scaleFactor.coerceIn(0.5f, 2f)
                applyZoom(factor, detector.focusX, detector.focusY)
                return true
            }
        }
    )

    // ─────────────────────────────────────────────────────────────────────────

    fun startRendering() { handler.post(refreshTick) }
    fun stopRendering()  { handler.removeCallbacks(refreshTick) }

    /** Hardware decode: set actual decoded video dimensions from MediaCodec output format. */
    fun setHardwareDimensions(w: Int, h: Int) {
        if (w == frameW && h == frameH) return
        frameW = w; frameH = h
        rebuildMatrix()
        invalidate()
    }

    fun setNaturalScroll(enabled: Boolean) {
        naturalScroll = enabled
    }

    private fun pullFrame() {
        if (isHardwareDecodeMode) {
            // Hardware mode: dimensions come from VideoDecoder.onDimensionsAvailable callback
            // (setHardwareDimensions). Here we just update gesture geometry from remoteGeometry().
            client.remoteGeometry()?.let { g ->
                if (g.width > 0 && g.height > 0) {
                    geomX = g.x; geomY = g.y; geomW = g.width; geomH = g.height
                }
            }
            if (cursorVisible) invalidate()
            return
        }

        val size = client.frameSize() ?: return
        val (w, h) = size
        if (w <= 0 || h <= 0) return
        if (w != frameW || h != frameH || pixels.size < w * h) {
            frameW = w; frameH = h
            pixels = IntArray(w * h)
            bitmap = Bitmap.createBitmap(w, h, Bitmap.Config.ARGB_8888)
            rebuildMatrix()
        }
        client.remoteGeometry()?.let { g ->
            if (g.width > 0 && g.height > 0) {
                geomX = g.x; geomY = g.y; geomW = g.width; geomH = g.height
            }
        }
        client.pollFrame(pixels) ?: return
        bitmap?.setPixels(pixels, 0, frameW, 0, 0, frameW, frameH)
        invalidate()
    }

    override fun onSizeChanged(w: Int, h: Int, oldw: Int, oldh: Int) {
        super.onSizeChanged(w, h, oldw, oldh)
        rebuildMatrix()
    }

    private fun rebuildMatrix() {
        if (frameW <= 0 || frameH <= 0 || width <= 0 || height <= 0) {
            Log.i("EvdGameScreen", "rebuildMatrix EARLY RETURN frameW=$frameW frameH=$frameH viewW=$width viewH=$height")
            return
        }
        baseFit = min(width.toFloat() / frameW, height.toFloat() / frameH)
            .coerceAtLeast(MIN_SCALE)
        val totalScale = baseFit * userZoom
        val drawW = frameW * totalScale
        val drawH = frameH * totalScale
        val tx = (width - drawW) / 2f + panX
        val ty = (height - drawH) / 2f + panY
        matrix.setScale(totalScale, totalScale)
        matrix.postTranslate(tx, ty)
        matrix.invert(matrixInv)
        Log.i("EvdGameScreen", "rebuildMatrix frameW=$frameW frameH=$frameH viewW=$width viewH=$height baseFit=$baseFit tx=$tx ty=$ty drawW=$drawW drawH=$drawH")
        onVideoRectChanged?.invoke(VideoRect(tx.toInt(), ty.toInt(), drawW.toInt(), drawH.toInt()))
    }

    override fun onDraw(canvas: Canvas) {
        if (isHardwareDecodeMode) {
            // TextureView is behind this view and shows the video — only draw cursor overlay.
            if (cursorVisible && cursorViewX >= 0f) {
                drawCursor(canvas, cursorViewX, cursorViewY)
            }
            return
        }
        canvas.drawColor(Color.BLACK)
        val bmp = bitmap ?: return
        canvas.drawBitmap(bmp, matrix, paint)
        if (cursorVisible && cursorViewX >= 0f) {
            drawCursor(canvas, cursorViewX, cursorViewY)
        }
    }

    private val cursorStrokePaint = Paint().apply {
        color = Color.BLACK; strokeWidth = 4f
        style = Paint.Style.STROKE; isAntiAlias = true
    }
    private val cursorFillPaint = Paint().apply {
        color = Color.WHITE; strokeWidth = 2f
        style = Paint.Style.STROKE; isAntiAlias = true
    }
    private val cursorDotPaint = Paint().apply {
        color = Color.WHITE; style = Paint.Style.FILL; isAntiAlias = true
    }

    private fun drawCursor(canvas: Canvas, cx: Float, cy: Float) {
        val arm = 14f * cursorScale
        val gap = 5f * cursorScale
        val dotR = 2.5f * cursorScale
        for (p in listOf(cursorStrokePaint, cursorFillPaint)) {
            canvas.drawLine(cx - arm, cy, cx - gap, cy, p)
            canvas.drawLine(cx + gap, cy, cx + arm, cy, p)
            canvas.drawLine(cx, cy - arm, cx, cy - gap, p)
            canvas.drawLine(cx, cy + gap, cx, cy + arm, p)
            canvas.drawCircle(cx, cy, 4f * cursorScale, p)
        }
        canvas.drawCircle(cx, cy, dotR, cursorDotPaint)
    }

    // ── жесты ────────────────────────────────────────────────────────────────

    override fun onTouchEvent(event: MotionEvent): Boolean {
        scaleDetector.onTouchEvent(event)
        if (!isGameMode) gestureDetector.onTouchEvent(event)

        when (event.actionMasked) {

            MotionEvent.ACTION_DOWN -> {
                longPressFired = false
                downViewX = event.x; downViewY = event.y
                prevFingerX = event.x; prevFingerY = event.y
                remoteAccumX = 0f; remoteAccumY = 0f
                lastMoveMs = SystemClock.elapsedRealtime()
                if (!isGameMode) {
                    val (cvx, cvy) = remoteToView(lastRemoteX, lastRemoteY)
                    showCursorAt(cvx, cvy)
                }
            }

            MotionEvent.ACTION_POINTER_DOWN -> {
                if (event.pointerCount == 2) {
                    twoFingerMode = true
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
                if (twoFingerMode && event.pointerCount >= 2) {
                    // In game mode allow pan only when zoomed in (local, no host events)
                    if (!isGameMode || userZoom > 1f) handleTwoFingerMove(event)
                } else if (!twoFingerMode && !isGameMode) {
                    handleOneFingerMove(event)
                }
            }

            MotionEvent.ACTION_POINTER_UP -> {
                if (twoFingerMode && event.pointerCount == 2) {
                    twoFingerEndTime = System.currentTimeMillis()
                    val elapsed = twoFingerEndTime - twoFingerDownTime

                    if (!isGameMode && !twoFingerMoved && elapsed < TWO_FINGER_TAP_MS) {
                        performHapticFeedback(HapticFeedbackConstants.LONG_PRESS)
                        client.rightClick(lastRemoteX, lastRemoteY)
                    }

                    val ri = if (event.actionIndex == 0) 1 else 0
                    prevFingerX = event.getX(ri); prevFingerY = event.getY(ri)
                    downViewX = prevFingerX; downViewY = prevFingerY
                    remoteAccumX = 0f; remoteAccumY = 0f
                    lastMoveMs = SystemClock.elapsedRealtime()
                    // Suppress phantom left-click in ACTION_UP after any 2-finger gesture
                    longPressFired = true

                    twoFingerMode = false
                    scrollAccumX = 0f; scrollAccumY = 0f
                }
            }

            MotionEvent.ACTION_UP -> {
                if (!twoFingerMode && !longPressFired && !isGameMode) {
                    val dist = hypot(event.x - downViewX, event.y - downViewY)
                    if (dist < tapSlop) {
                        if (rightClickCallback?.invoke(lastRemoteX, lastRemoteY) != true) {
                            client.touch(lastRemoteX, lastRemoteY, 0)
                            client.touch(lastRemoteX, lastRemoteY, 2)
                        }
                    }
                }
                twoFingerMode = false
                longPressFired = false
            }

            MotionEvent.ACTION_CANCEL -> {
                twoFingerMode = false
                longPressFired = false
                twoFingerEndTime = System.currentTimeMillis()
                scrollAccumX = 0f; scrollAccumY = 0f
            }
        }
        return true
    }

    // ── 1-палец: sub-pixel аккумулятор + shake-to-find ───────────────────────

    private fun handleOneFingerMove(event: MotionEvent) {
        val totalScale = (baseFit * userZoom).coerceAtLeast(MIN_SCALE)

        // Velocity for shake-to-find (measured in touch pixels/s, not remote)
        val now = SystemClock.elapsedRealtime()
        val dtMs = (now - lastMoveMs).coerceAtLeast(1).toFloat()
        val touchDx = event.x - prevFingerX
        val touchDy = event.y - prevFingerY
        val speed = hypot(touchDx, touchDy) / dtMs * 1000f
        lastMoveMs = now
        if (speed > SHAKE_SPEED_PX_PER_SEC) {
            cursorScale = SHAKE_SCALE
            handler.removeCallbacks(resetCursorScaleRunnable)
            handler.postDelayed(resetCursorScaleRunnable, SHAKE_HOLD_MS)
        }

        // Sub-pixel accumulator: collect fractional remote pixels across frames
        remoteAccumX += touchDx / totalScale
        remoteAccumY += touchDy / totalScale
        prevFingerX = event.x; prevFingerY = event.y

        val dx = remoteAccumX.toInt()
        val dy = remoteAccumY.toInt()
        remoteAccumX -= dx
        remoteAccumY -= dy

        val clampX0 = if (geomW > 0) geomX else 0
        val clampX1 = if (geomW > 0) geomX + geomW - 1 else (frameW - 1).coerceAtLeast(0)
        val clampY0 = if (geomH > 0) geomY else 0
        val clampY1 = if (geomH > 0) geomY + geomH - 1 else (frameH - 1).coerceAtLeast(0)

        val newRx = (lastRemoteX + dx).coerceIn(clampX0, clampX1)
        val newRy = (lastRemoteY + dy).coerceIn(clampY0, clampY1)

        if (newRx != lastRemoteX || newRy != lastRemoteY) {
            lastRemoteX = newRx; lastRemoteY = newRy
            client.touch(lastRemoteX, lastRemoteY, 1)
        }
        if (userZoom > 1f) followCursor()
        val (cvx, cvy) = remoteToView(lastRemoteX, lastRemoteY)
        showCursorAt(cvx, cvy)
    }

    // ── 2-пальца: скролл с axis-lock или пан ─────────────────────────────────

    private fun handleTwoFingerMove(event: MotionEvent) {
        val midX = (event.getX(0) + event.getX(1)) / 2f
        val midY = (event.getY(0) + event.getY(1)) / 2f
        val dx = midX - prevMidX
        val dy = midY - prevMidY

        val totalDrift = hypot(midX - twoFingerDownMidX, midY - twoFingerDownMidY)
        if (totalDrift > TWO_FINGER_TAP_SLOP) twoFingerMoved = true

        if (userZoom > 1f) {
            if (dx != 0f || dy != 0f) applyPan(dx, dy)
        } else {
            scrollAccumX += dx
            scrollAccumY += dy

            // Lock to dominant axis to eliminate diagonal scroll drift
            if (!scrollAxisLocked) {
                val totalMovement = abs(scrollAccumX) + abs(scrollAccumY)
                if (totalMovement > AXIS_LOCK_THRESHOLD_DP * density) {
                    scrollIsVertical = abs(scrollAccumY) >= abs(scrollAccumX)
                    scrollAxisLocked = true
                    if (scrollIsVertical) scrollAccumX = 0f else scrollAccumY = 0f
                }
            }

            if (scrollAxisLocked && scrollIsVertical) {
                val steps = (scrollAccumY / SCROLL_STEP_PX).toInt()
                if (steps != 0) {
                    scrollAccumY -= steps * SCROLL_STEP_PX
                    val delta = if (naturalScroll) -steps else steps
                    client.scroll(lastRemoteX, lastRemoteY, delta)
                }
            }

            // Keep cursor visible while scrolling
            showCursorAt(cursorViewX.coerceAtLeast(0f), cursorViewY.coerceAtLeast(0f))
        }

        prevMidX = midX; prevMidY = midY
    }

    // ── зум / пан helpers ────────────────────────────────────────────────────

    private fun applyZoom(factor: Float, focusX: Float, focusY: Float) {
        val safeF = factor.coerceIn(0.5f, 2f)
        val newZoom = (userZoom * safeF).coerceIn(1f, 4f)
        if (newZoom == userZoom) return

        val oldScale = (baseFit * userZoom).coerceAtLeast(MIN_SCALE)
        val newScale = (baseFit * newZoom).coerceAtLeast(MIN_SCALE)
        val drawW0 = frameW * oldScale
        val drawH0 = frameH * oldScale
        val tx0 = (width - drawW0) / 2f + panX
        val ty0 = (height - drawH0) / 2f + panY

        val drawW1 = frameW * newScale
        val drawH1 = frameH * newScale
        val tx1 = focusX - (focusX - tx0) * newScale / oldScale
        val ty1 = focusY - (focusY - ty0) * newScale / oldScale

        userZoom = newZoom
        panX = tx1 - (width - drawW1) / 2f
        panY = ty1 - (height - drawH1) / 2f
        clampPan()
        rebuildMatrix()
        invalidate()
    }

    private fun applyPan(dx: Float, dy: Float) {
        panX += dx; panY += dy
        clampPan()
        rebuildMatrix()
        invalidate()
    }

    private fun clampPan() {
        val totalScale = (baseFit * userZoom).coerceAtLeast(MIN_SCALE)
        val drawW = frameW * totalScale
        val drawH = frameH * totalScale
        val maxPanX = max(0f, (drawW - width) / 2f)
        val maxPanY = max(0f, (drawH - height) / 2f)
        panX = panX.coerceIn(-maxPanX, maxPanX)
        panY = panY.coerceIn(-maxPanY, maxPanY)
    }

    fun resetZoom() {
        userZoom = 1f; panX = 0f; panY = 0f
        rebuildMatrix(); invalidate()
    }

    private fun followCursor() {
        val margin = 48f * density
        val (cvx, cvy) = remoteToView(lastRemoteX, lastRemoteY)
        var changed = false
        if (cvx < margin) { panX += margin - cvx; changed = true }
        else if (cvx > width - margin) { panX -= cvx - (width - margin); changed = true }
        if (cvy < margin) { panY += margin - cvy; changed = true }
        else if (cvy > height - margin) { panY -= cvy - (height - margin); changed = true }
        if (changed) { clampPan(); rebuildMatrix() }
    }

    // ── курсор ───────────────────────────────────────────────────────────────

    private fun showCursorAt(vx: Float, vy: Float) {
        cursorViewX = vx; cursorViewY = vy
        cursorVisible = true
        handler.removeCallbacks(hideCursorRunnable)
        handler.postDelayed(hideCursorRunnable, 2000)
        invalidate()
    }

    // ── координаты ──────────────────────────────────────────────────────────

    private fun viewToRemote(vx: Float, vy: Float): Pair<Int, Int> {
        val pts = floatArrayOf(vx, vy)
        matrixInv.mapPoints(pts)
        val rx = pts[0].roundToInt().coerceIn(0, (frameW - 1).coerceAtLeast(0))
        val ry = pts[1].roundToInt().coerceIn(0, (frameH - 1).coerceAtLeast(0))
        return Pair(rx, ry)
    }

    private fun remoteToView(rx: Int, ry: Int): Pair<Float, Float> {
        val pts = floatArrayOf(rx.toFloat(), ry.toFloat())
        matrix.mapPoints(pts)
        return Pair(pts[0], pts[1])
    }
}
