package ru.everty.desklite

import android.content.Context
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Matrix
import android.graphics.Paint
import android.os.Handler
import android.os.Looper
import android.view.GestureDetector
import android.view.MotionEvent
import android.view.ScaleGestureDetector
import android.view.View
import android.view.ViewConfiguration
import kotlin.math.hypot
import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToInt

/**
 * Удалённый экран — trackpad-режим (как в RustDesk):
 *  • 1 палец move          → MouseMove (курсор без нажатия)
 *  • 1 палец tap           → левый клик (down+up на месте касания)
 *  • Long press            → правый клик (вибрация)
 *  • 2 пальца pinch        → зум
 *  • 2 пальца drag         → пан по зумленной картинке
 *  • 2 пальца scroll       → колесо мыши
 */
class RemoteView(context: Context, private val client: NativeClient) : View(context) {

    // ── кадр ─────────────────────────────────────────────────────────────────
    private var bitmap: Bitmap? = null
    private var frameW = 0
    private var frameH = 0
    private var pixels = IntArray(0)
    private val paint = Paint(Paint.FILTER_BITMAP_FLAG)

    // ── трансформ (зум + пан) ────────────────────────────────────────────────
    // matrix = scale + translate; применяется к bitmap при рисовании и
    // инвертируется при пересчёте тач-координат.
    private val matrix = Matrix()
    private val matrixInv = Matrix()

    // базовый масштаб «вписать в экран»; реальный = baseFit * userZoom
    private var baseFit = 1f
    private var userZoom = 1f        // 1.0 = fit, макс 4.0, мин 1.0
    private var panX = 0f
    private var panY = 0f

    // Порог (px): если палец не сдвинулся дальше — это тап, иначе — drag.
    private val tapSlop = ViewConfiguration.get(context).scaledTouchSlop.toFloat()

    // ── состояние курсора (трекпад-режим) ───────────────────────────────────
    // Текущая позиция курсора на УДАЛЁННОМ экране. Обновляется дельтами.
    private var lastRemoteX = 0
    private var lastRemoteY = 0
    // Позиция пальца в предыдущем событии (для вычисления дельты движения)
    private var prevFingerX = 0f
    private var prevFingerY = 0f
    // Координаты VIEW при ACTION_DOWN (для различия тапа и drag)
    private var downViewX = 0f
    private var downViewY = 0f
    // Long press уже сработал → не посылать клик при поднятии пальца
    private var longPressFired = false
    // Позиция курсора в view-координатах для отрисовки (обновляется при каждом move)
    private var cursorViewX = -1f
    private var cursorViewY = -1f
    // Скрываем курсор через 2 сек после последнего касания
    private var cursorVisible = false
    private val hideCursorRunnable = Runnable { cursorVisible = false; invalidate() }

    // ── двухпальцевый режим ──────────────────────────────────────────────────
    private var twoFingerMode = false
    private var prevSpan = 0f
    private var prevMidX = 0f
    private var prevMidY = 0f

    // ── рендер-тик ──────────────────────────────────────────────────────────
    private val handler = Handler(Looper.getMainLooper())
    private val refreshTick = object : Runnable {
        override fun run() { pullFrame(); handler.postDelayed(this, 16) }
    }

    // ── callback для режима «следующий тап = правый клик» из MainActivity ───
    private var rightClickCallback: ((Int, Int) -> Boolean)? = null
    fun setRightClickCallback(cb: (Int, Int) -> Boolean) { rightClickCallback = cb }

    // ── детекторы жестов ────────────────────────────────────────────────────
    private val gestureDetector = GestureDetector(context,
        object : GestureDetector.SimpleOnGestureListener() {
            override fun onLongPress(e: MotionEvent) {
                if (twoFingerMode) return
                longPressFired = true
                // Трекпад: правый клик там где стоит курсор, не где палец
                performHapticFeedback(android.view.HapticFeedbackConstants.LONG_PRESS)
                client.rightClick(lastRemoteX, lastRemoteY)
            }
        }
    )

    private val scaleDetector = ScaleGestureDetector(context,
        object : ScaleGestureDetector.SimpleOnScaleGestureListener() {
            override fun onScale(detector: ScaleGestureDetector): Boolean {
                val factor = detector.scaleFactor
                val focusX = detector.focusX
                val focusY = detector.focusY
                applyZoom(factor, focusX, focusY)
                return true
            }
        }
    )

    // ─────────────────────────────────────────────────────────────────────────

    fun startRendering() { handler.post(refreshTick) }
    fun stopRendering()  { handler.removeCallbacks(refreshTick) }

    private fun pullFrame() {
        val size = client.frameSize() ?: return
        val (w, h) = size
        if (w <= 0 || h <= 0) return
        if (w != frameW || h != frameH || pixels.size < w * h) {
            frameW = w; frameH = h
            pixels = IntArray(w * h)
            bitmap = Bitmap.createBitmap(w, h, Bitmap.Config.ARGB_8888)
            rebuildMatrix()
        }
        client.pollFrame(pixels) ?: return
        bitmap?.setPixels(pixels, 0, frameW, 0, 0, frameW, frameH)
        invalidate()
    }

    override fun onSizeChanged(w: Int, h: Int, oldw: Int, oldh: Int) {
        super.onSizeChanged(w, h, oldw, oldh)
        rebuildMatrix()
    }

    /** Пересчитать матрицу из baseFit, userZoom, panX, panY */
    private fun rebuildMatrix() {
        if (frameW == 0 || frameH == 0 || width == 0 || height == 0) return
        baseFit = min(width.toFloat() / frameW, height.toFloat() / frameH)
        val totalScale = baseFit * userZoom
        val drawW = frameW * totalScale
        val drawH = frameH * totalScale
        // центрируем + применяем пан
        val tx = (width - drawW) / 2f + panX
        val ty = (height - drawH) / 2f + panY
        matrix.setScale(totalScale, totalScale)
        matrix.postTranslate(tx, ty)
        matrix.invert(matrixInv)
    }

    override fun onDraw(canvas: Canvas) {
        canvas.drawColor(Color.BLACK)
        val bmp = bitmap ?: return
        canvas.drawBitmap(bmp, matrix, paint)

        // Курсор: рисуем всегда пока видим (исчезает через 2 сек после касания)
        if (cursorVisible && cursorViewX >= 0f) {
            drawCursor(canvas, cursorViewX, cursorViewY)
        }
    }

    // Курсор: белый с чёрной обводкой — виден на любом фоне
    private val cursorStrokePaint = Paint().apply {
        color = Color.BLACK
        strokeWidth = 4f
        style = Paint.Style.STROKE
        isAntiAlias = true
    }
    private val cursorFillPaint = Paint().apply {
        color = Color.WHITE
        strokeWidth = 2f
        style = Paint.Style.STROKE
        isAntiAlias = true
    }
    private val cursorDotPaint = Paint().apply {
        color = Color.WHITE
        style = Paint.Style.FILL
        isAntiAlias = true
    }

    private fun drawCursor(canvas: Canvas, cx: Float, cy: Float) {
        val arm = 14f
        val gap = 5f
        // Чёрная обводка (рисуем первой — она шире)
        for (p in listOf(cursorStrokePaint, cursorFillPaint)) {
            canvas.drawLine(cx - arm, cy, cx - gap, cy, p)
            canvas.drawLine(cx + gap, cy, cx + arm, cy, p)
            canvas.drawLine(cx, cy - arm, cx, cy - gap, p)
            canvas.drawLine(cx, cy + gap, cx, cy + arm, p)
            canvas.drawCircle(cx, cy, 4f, p)
        }
        // Центральная точка
        canvas.drawCircle(cx, cy, 2.5f, cursorDotPaint)
    }

    // ── жесты ────────────────────────────────────────────────────────────────

    override fun onTouchEvent(event: MotionEvent): Boolean {
        scaleDetector.onTouchEvent(event)
        gestureDetector.onTouchEvent(event)

        val pointerCount = event.pointerCount

        when (event.actionMasked) {

            MotionEvent.ACTION_POINTER_DOWN -> {
                if (pointerCount == 2) {
                    twoFingerMode = true
                    longPressFired = false
                    prevMidX = (event.getX(0) + event.getX(1)) / 2f
                    prevMidY = (event.getY(0) + event.getY(1)) / 2f
                    prevSpan = span(event)
                }
            }

            MotionEvent.ACTION_POINTER_UP -> {
                if (pointerCount <= 2) {
                    twoFingerMode = false
                }
            }

            MotionEvent.ACTION_DOWN -> {
                if (!twoFingerMode) {
                    longPressFired = false
                    downViewX = event.x; downViewY = event.y
                    prevFingerX = event.x; prevFingerY = event.y
                    // Трекпад: при касании курсор НЕ прыгает, остаётся где был
                    val (cvx, cvy) = remoteToView(lastRemoteX, lastRemoteY)
                    showCursorAt(cvx, cvy)
                }
            }

            MotionEvent.ACTION_MOVE -> {
                if (twoFingerMode && pointerCount >= 2) {
                    val midX = (event.getX(0) + event.getX(1)) / 2f
                    val midY = (event.getY(0) + event.getY(1)) / 2f
                    val curSpan = span(event)

                    if (prevSpan > 0f) {
                        applyZoom(curSpan / prevSpan, midX, midY)
                    }
                    val dx = midX - prevMidX
                    val dy = midY - prevMidY
                    if (dx != 0f || dy != 0f) applyPan(dx, dy)

                    prevMidX = midX; prevMidY = midY
                    prevSpan = curSpan

                    val scrollSteps = (dy / 40f).roundToInt()
                    if (scrollSteps != 0) {
                        val (rx, ry) = viewToRemote(midX, midY)
                        client.scroll(rx, ry, -scrollSteps)
                    }
                } else if (!twoFingerMode) {
                    // Трекпад: дельта пальца → дельта курсора (без абсолютного прыжка)
                    val totalScale = baseFit * userZoom
                    val dx = (event.x - prevFingerX) / totalScale
                    val dy = (event.y - prevFingerY) / totalScale
                    prevFingerX = event.x; prevFingerY = event.y
                    val newRx = (lastRemoteX + dx).toInt().coerceIn(0, frameW - 1)
                    val newRy = (lastRemoteY + dy).toInt().coerceIn(0, frameH - 1)
                    if (newRx != lastRemoteX || newRy != lastRemoteY) {
                        lastRemoteX = newRx; lastRemoteY = newRy
                        client.touch(lastRemoteX, lastRemoteY, 1)
                    }
                    // При зуме двигаем картинку за курсором, чтобы он всегда был виден
                    if (userZoom > 1f) followCursor()
                    val (cvx, cvy) = remoteToView(lastRemoteX, lastRemoteY)
                    showCursorAt(cvx, cvy)
                }
            }

            MotionEvent.ACTION_UP -> {
                if (!twoFingerMode && !longPressFired) {
                    val dist = hypot(event.x - downViewX, event.y - downViewY)
                    if (dist < tapSlop) {
                        // Тап: кликаем там где стоит курсор (не где был палец)
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
            }
        }
        return true
    }

    // ── зум / пан helpers ────────────────────────────────────────────────────

    private fun applyZoom(factor: Float, focusX: Float, focusY: Float) {
        val newZoom = (userZoom * factor).coerceIn(1f, 4f)
        if (newZoom == userZoom) return
        // Сохраняем точку фокуса на экране при изменении масштаба
        val oldScale = baseFit * userZoom
        val newScale = baseFit * newZoom
        val drawW0 = frameW * oldScale
        val drawH0 = frameH * oldScale
        val tx0 = (width - drawW0) / 2f + panX
        val ty0 = (height - drawH0) / 2f + panY

        // focusX в системе bitmap: (focusX - tx0) / oldScale
        // После масштаба та же точка bitmap должна быть под фокусом
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

    /** Не даём уйти за края — при userZoom=1 пан=0 */
    private fun clampPan() {
        val totalScale = baseFit * userZoom
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

    /**
     * Сдвигаем пан так, чтобы курсор всегда оставался в зоне видимости
     * (с отступом [margin] от краёв). Вызывается только при userZoom > 1.
     */
    private fun followCursor() {
        val margin = 48f * resources.displayMetrics.density
        val (cvx, cvy) = remoteToView(lastRemoteX, lastRemoteY)
        var changed = false
        if (cvx < margin) {
            panX += margin - cvx; changed = true
        } else if (cvx > width - margin) {
            panX -= cvx - (width - margin); changed = true
        }
        if (cvy < margin) {
            panY += margin - cvy; changed = true
        } else if (cvy > height - margin) {
            panY -= cvy - (height - margin); changed = true
        }
        if (changed) {
            clampPan()
            rebuildMatrix()
        }
    }

    // ── курсор ───────────────────────────────────────────────────────────────

    private fun showCursorAt(vx: Float, vy: Float) {
        cursorViewX = vx
        cursorViewY = vy
        cursorVisible = true
        // Перезапускаем таймер исчезновения
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

    /** Позиция удалённого курсора в координатах view (для отрисовки crosshair). */
    private fun remoteToView(rx: Int, ry: Int): Pair<Float, Float> {
        val pts = floatArrayOf(rx.toFloat(), ry.toFloat())
        matrix.mapPoints(pts)
        return Pair(pts[0], pts[1])
    }

    private fun span(e: MotionEvent): Float {
        val dx = e.getX(0) - e.getX(1)
        val dy = e.getY(0) - e.getY(1)
        return Math.hypot(dx.toDouble(), dy.toDouble()).toFloat()
    }
}
