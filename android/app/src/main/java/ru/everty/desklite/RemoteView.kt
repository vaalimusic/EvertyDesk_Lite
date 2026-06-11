package ru.everty.desklite

import android.content.Context
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Matrix
import android.graphics.Paint
import android.graphics.RectF
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
import kotlin.math.sqrt

/**
 * Удалённый экран с жестами:
 *  • 1 палец tap           → левый клик
 *  • 1 палец drag          → мышь с зажатой левой кнопкой
 *  • Long press            → правый клик (вибрация)
 *  • 2 пальца pinch        → зум
 *  • 2 пальца drag         → пан по зумленной картинке
 *  • 2 пальца scroll (fling вертикаль) → колесо мыши
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

    // Системный порог drag: палец должен сдвинуться дальше этого (px) чтобы
    // начать drag. Защищает от случайного MouseDown при обычном тапе.
    private val touchSlop = ViewConfiguration.get(context).scaledTouchSlop.toFloat()

    // ── состояние мыши + курсор ──────────────────────────────────────────────
    private var mouseDown = false
    private var lastRemoteX = 0
    private var lastRemoteY = 0
    // Координаты пальца в момент ACTION_DOWN (для измерения дистанции drag)
    private var downViewX = 0f
    private var downViewY = 0f
    private var dragStarted = false   // true как только превысили touchSlop
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

            override fun onSingleTapUp(e: MotionEvent): Boolean {
                if (twoFingerMode) return false
                val (rx, ry) = viewToRemote(e.x, e.y)
                showCursorAt(e.x, e.y)
                // Проверяем режим «следующий тап = правый клик»
                if (rightClickCallback?.invoke(rx, ry) == true) return true
                client.touch(rx, ry, 0)
                client.touch(rx, ry, 2)
                return true
            }

            override fun onLongPress(e: MotionEvent) {
                if (twoFingerMode) return
                val (rx, ry) = viewToRemote(e.x, e.y)
                showCursorAt(e.x, e.y)
                performHapticFeedback(android.view.HapticFeedbackConstants.LONG_PRESS)
                client.rightClick(rx, ry)
            }

            override fun onScroll(
                e1: MotionEvent?, e2: MotionEvent,
                distanceX: Float, distanceY: Float
            ): Boolean = false  // обрабатываем вручную в onTouchEvent
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
                // Второй палец — переходим в режим зума/пана
                if (pointerCount == 2) {
                    twoFingerMode = true
                    // Если была зажата мышь — отпускаем
                    if (mouseDown) {
                        client.touch(lastRemoteX, lastRemoteY, 2)
                        mouseDown = false
                    }
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
                    val (rx, ry) = viewToRemote(event.x, event.y)
                    lastRemoteX = rx; lastRemoteY = ry
                    downViewX = event.x; downViewY = event.y
                    dragStarted = false
                    showCursorAt(event.x, event.y)
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
                    val dist = hypot(event.x - downViewX, event.y - downViewY)
                    // Начинаем drag только после превышения порога touchSlop
                    if (!dragStarted && dist > touchSlop) {
                        dragStarted = true
                        // MouseDown в начальной точке касания (не там где уже сдвинулись)
                        client.touch(lastRemoteX, lastRemoteY, 0)
                        mouseDown = true
                    }
                    if (dragStarted) {
                        val (rx, ry) = viewToRemote(event.x, event.y)
                        showCursorAt(event.x, event.y)
                        if (rx != lastRemoteX || ry != lastRemoteY) {
                            client.touch(rx, ry, 1)
                            lastRemoteX = rx; lastRemoteY = ry
                        }
                    }
                }
            }

            MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> {
                twoFingerMode = false
                dragStarted = false
                if (mouseDown) {
                    client.touch(lastRemoteX, lastRemoteY, 2)
                    mouseDown = false
                }
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

    private fun span(e: MotionEvent): Float {
        val dx = e.getX(0) - e.getX(1)
        val dy = e.getY(0) - e.getY(1)
        return Math.hypot(dx.toDouble(), dy.toDouble()).toFloat()
    }
}
