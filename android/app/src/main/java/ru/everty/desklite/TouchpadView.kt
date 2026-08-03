package ru.everty.desklite

import android.content.Context
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.graphics.PointF
import android.graphics.RectF
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.view.Choreographer
import android.view.GestureDetector
import android.view.HapticFeedbackConstants
import android.view.MotionEvent
import android.view.View
import android.view.ViewConfiguration
import kotlin.math.abs
import kotlin.math.hypot

/**
 * Тачпад без картинки (слепой режим):
 *  • 1 палец move          → курсор (MouseMove)
 *  • 1 палец tap           → левый клик
 *  • 1 палец long press    → drag-select (зажать ЛКМ + двигать для выделения)
 *  • 2 пальца tap < 500ms  → правый клик (Mac-style)
 *  • 2 пальца drag         → вертикальный скролл (с axis-lock)
 *  • 3 пальца свайп ←/→    → назад / вперёд (как на Mac)
 *
 * Курсор на экране НЕ рисуется (он был не под пальцем и путал). Вместо этого
 * под пальцами рисуются мягкие следы касаний, затухающие после отпускания.
 */
class TouchpadView(context: Context, private val client: NativeClient) : View(context) {

    private val density = resources.displayMetrics.density
    private val tapSlop = ViewConfiguration.get(context).scaledTouchSlop.toFloat()
    private val handler = Handler(Looper.getMainLooper())

    // ── Paints ────────────────────────────────────────────────────────────────
    private val panelPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = Color.rgb(14, 18, 16)
    }
    private val linePaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = Color.rgb(32, 46, 39)
        strokeWidth = 1f * density
    }
    private val titlePaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = Color.WHITE
        textSize = 17f * density
        isFakeBoldText = true
    }
    private val hintPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = Color.rgb(190, 205, 198)
        textSize = 12f * density
    }
    private val hintKeyPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = Color.rgb(18, 201, 114)
        textSize = 12f * density
        isFakeBoldText = true
    }
    // Мягкий след под активным пальцем (заливка + свечение).
    private val touchFillPaint = Paint(Paint.ANTI_ALIAS_FLAG)
    private val touchGlowPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        style = Paint.Style.STROKE
        strokeWidth = 2f * density
    }
    private val navBadgeBgPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = Color.argb(0xE6, 0x12, 0xC9, 0x72)
    }
    private val navBadgeTextPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = Color.WHITE
        textSize = 15f * density
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

    // Сегрегация жестов: как только в текущем касании участвовало ≥2 пальцев,
    // одиночные действия (движение курсора, тап-клик) блокируются до полного
    // отрыва ВСЕХ пальцев. Иначе остаточный палец после скролла/3-пальцевого
    // свайпа даёт фантомный клик (#3) или скачок курсора (#2).
    private var gestureConsumed = false

    private var lastMoveMs = 0L

    // ── Движок курсора: ускорение + сглаживание + отправка по VSync ────────────
    // Ускорение (как на трекпаде Mac): медленное движение пальца → близко к
    // 1:1 (снайперская точность на мелких целях), быстрое → множитель растёт
    // (быстрый перелёт через весь экран). Плавность: дельты копятся и
    // отправляются один раз за кадр дисплея (Choreographer/VSync), а не на
    // каждый MotionEvent — меньше пакетов, ровнее курсор. Джиттер убирается
    // адаптивным сглаживанием (аналог 1€-фильтра): на медленном движении
    // сглаживаем сильнее, на быстром — почти не трогаем (без задержки).
    private var accelEnabled = true
    private var lastMoveTimeNs = 0L
    private var smoothedSpeed = 0f   // px/ms, низкочастотно сглаженная скорость
    private var smoothDx = 0f        // сглаженная дельта X (для антиджиттера)
    private var smoothDy = 0f
    private var pendingDx = 0f       // накопленное смещение до ближайшего VSync
    private var pendingDy = 0f
    private var frameScheduled = false
    private val choreographer = Choreographer.getInstance()
    private val frameCallback = Choreographer.FrameCallback { flushMotion() }

    // ── Следы касаний ─────────────────────────────────────────────────────────
    // Активные пальцы: pointerId → текущая позиция во view-координатах.
    private val activeTouches = LinkedHashMap<Int, PointF>()
    // Затухающие следы: позиция + прогресс жизни (1 → свежий, 0 → исчез).
    private class TrailDot(var x: Float, var y: Float, var life: Float, val right: Boolean)
    private val trail = ArrayDeque<TrailDot>()
    private var trailAnimating = false
    private val trailAnim = object : Runnable {
        override fun run() {
            val it = trail.iterator()
            while (it.hasNext()) {
                val d = it.next()
                d.life -= 0.06f
                if (d.life <= 0f) it.remove()
            }
            invalidate()
            if (trail.isNotEmpty() || activeTouches.isNotEmpty()) {
                handler.postDelayed(this, 16)
            } else {
                trailAnimating = false
            }
        }
    }
    private fun ensureTrailAnim() {
        if (!trailAnimating) { trailAnimating = true; handler.post(trailAnim) }
    }

    // ── 3 пальца: навигация назад/вперёд ──────────────────────────────────────
    private var threeFinger = false
    private var threeDownX = 0f
    private var threeFired = false
    private var navBadge = ""       // "◀ Назад" / "Вперёд ▶"
    private var navBadgeLife = 0f
    private var threeEndTime = 0L

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

    private val threeFingerNavThreshold get() = 90f * density  // px свайпа для навигации

    companion object {
        private const val TWO_FINGER_TAP_MS = 500L
        private const val TWO_FINGER_TAP_SLOP = 80f
        private const val TWO_FINGER_COOLDOWN = 500L
        // Порог свайпа тремя пальцами (в density-независимых пикселях считается в поле).
        private const val THREE_FINGER_NAV_THRESHOLD_DP = 90f

        // Кривая ускорения курсора.
        private const val ACCEL_SPEED_REF = 2.2f   // px/мс — скорость «полного» ускорения
        private const val ACCEL_MIN_GAIN = 0.55f   // множитель на медленном движении (точность)
        private const val ACCEL_MAX_GAIN = 2.6f    // множитель на быстром рывке (перелёт экрана)
    }

    // ── Click feedback: тактильная отдача + яркий отпечаток под пальцем ────────
    private fun triggerClick(isRight: Boolean) {
        // Яркий след в месте последнего активного касания.
        activeTouches.values.lastOrNull()?.let { p ->
            trail.addLast(TrailDot(p.x, p.y, 1f, isRight))
            ensureTrailAnim()
        }
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

        // ── Компактный хедер: заголовок + две строки коротких подсказок ──────
        val hx = panel.left + 18f * density
        canvas.drawText(
            if (dragSelectMode) "Выделение текста" else "Тачпад",
            hx, panel.top + 34f * density, titlePaint,
        )
        // Подсказки: две строки, ключевые слова акцентным цветом. Помещаются
        // на любой ширине, потому что короткие и разбиты по строкам.
        drawHintLine(canvas, hx, panel.top + 60f * density, listOf(
            "Тап" to true, " — клик   " to false,
            "2 пальца" to true, " — скролл / ПКМ" to false,
        ))
        drawHintLine(canvas, hx, panel.top + 80f * density, listOf(
            "3 пальца ←/→" to true, " — назад / вперёд   " to false,
            "Удержание" to true, " — выделение" to false,
        ))
        canvas.drawLine(
            panel.left, panel.top + 94f * density,
            panel.right, panel.top + 94f * density,
            linePaint,
        )

        // ── Следы касаний (затухающие) ───────────────────────────────────────
        for (d in trail) {
            val base = if (d.right) Color.rgb(0xFF, 0x99, 0x33) else Color.rgb(0x12, 0xC9, 0x72)
            val r = (18f + (1f - d.life) * 26f) * density   // расширяется по мере затухания
            touchGlowPaint.color = Color.argb((d.life * 200).toInt(),
                Color.red(base), Color.green(base), Color.blue(base))
            canvas.drawCircle(d.x, d.y, r, touchGlowPaint)
        }

        // ── Активные пальцы (яркие мягкие круги под пальцем) ─────────────────
        for (p in activeTouches.values) {
            touchFillPaint.color = Color.argb(0x33, 0x12, 0xC9, 0x72)
            canvas.drawCircle(p.x, p.y, 26f * density, touchFillPaint)
            touchGlowPaint.color = Color.argb(0xCC, 0x12, 0xC9, 0x72)
            canvas.drawCircle(p.x, p.y, 26f * density, touchGlowPaint)
        }

        // ── Бейдж навигации (3 пальца) ────────────────────────────────────────
        if (navBadgeLife > 0f && navBadge.isNotEmpty()) {
            val tw = navBadgeTextPaint.measureText(navBadge)
            val bp = 16f * density
            val cx = panel.centerX()
            val cy = panel.centerY()
            navBadgeBgPaint.alpha = (navBadgeLife * 0xE6).toInt()
            canvas.drawRoundRect(
                RectF(cx - tw / 2 - bp, cy - 22f * density, cx + tw / 2 + bp, cy + 18f * density),
                14f * density, 14f * density, navBadgeBgPaint,
            )
            navBadgeTextPaint.alpha = (navBadgeLife * 255).toInt()
            canvas.drawText(navBadge, cx - tw / 2, cy + 8f * density, navBadgeTextPaint)
        }
    }

    /** Рисует строку подсказки с чередованием акцентных/обычных сегментов. */
    private fun drawHintLine(canvas: Canvas, startX: Float, y: Float, parts: List<Pair<String, Boolean>>) {
        var x = startX
        for ((text, isKey) in parts) {
            val p = if (isKey) hintKeyPaint else hintPaint
            canvas.drawText(text, x, y, p)
            x += p.measureText(text)
        }
    }

    private fun showNavBadge(forward: Boolean) {
        navBadge = if (forward) "Вперёд ▶" else "◀ Назад"
        navBadgeLife = 1f
        val fade = object : Runnable {
            override fun run() {
                navBadgeLife -= 0.05f
                invalidate()
                if (navBadgeLife > 0f) handler.postDelayed(this, 16)
            }
        }
        handler.postDelayed(fade, 350)  // подержать, потом плавно убрать
    }

    // ── Touch ─────────────────────────────────────────────────────────────────

    // Защита от случайных касаний краёв (захват ладонью, свайп-бары системы).
    private val edgeGuardPx get() = 10f * density

    private fun isEdgeTouch(x: Float, y: Float): Boolean =
        x < edgeGuardPx || x > width - edgeGuardPx ||
        y < edgeGuardPx || y > height - edgeGuardPx

    override fun onTouchEvent(event: MotionEvent): Boolean {
        refreshRemoteSize()
        // Игнорируем жест только тремя пальцами в GestureDetector (long-press).
        if (!threeFinger) gestureDetector.onTouchEvent(event)

        // Обновляем позиции активных пальцев для отрисовки следов.
        syncActiveTouches(event)

        when (event.actionMasked) {

            MotionEvent.ACTION_DOWN -> {
                // Edge-guard: касание у самого края экрана — вероятно случайное.
                if (isEdgeTouch(event.x, event.y)) return true
                longPressFired = false
                dragSelectMode = false
                twoFinger = false
                threeFinger = false
                gestureConsumed = false
                downX = event.x; downY = event.y
                prevX = event.x; prevY = event.y
                resetMotionEngine()
                lastMoveMs = SystemClock.elapsedRealtime()
            }

            MotionEvent.ACTION_POINTER_DOWN -> {
                // Любой второй/третий палец делает жест «мультитач» — одиночные
                // действия отключаются до полного отпускания.
                gestureConsumed = true
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
                } else if (event.pointerCount == 3) {
                    // Переход к 3-пальцевому жесту: гасим скролл/выделение.
                    threeFinger = true
                    threeFired = false
                    twoFinger = false
                    if (dragSelectMode) {
                        client.touch(cursorX, cursorY, 2)
                        dragSelectMode = false
                    }
                    threeDownX = (0 until 3).sumOf { event.getX(it).toDouble() }.toFloat() / 3f
                }
            }

            MotionEvent.ACTION_MOVE -> {
                if (threeFinger && event.pointerCount >= 3) {
                    handleThreeFingerMove(event)
                } else if (twoFinger && event.pointerCount >= 2) {
                    handleTwoFingerMove(event)
                } else if (!twoFinger && !threeFinger && !gestureConsumed) {
                    // Одиночный курсор — только если в этом касании ещё не было
                    // мультитача (иначе остаточный палец сбивал бы курсор).
                    handleOneFingerMove(event)
                }
            }

            MotionEvent.ACTION_POINTER_UP -> {
                if (threeFinger) {
                    // Один палец из трёх поднят — жест завершён, ничего не кликаем.
                    if (event.pointerCount <= 3) {
                        threeFinger = false
                        threeEndTime = System.currentTimeMillis()
                    }
                } else if (twoFinger && event.pointerCount == 2) {
                    twoFingerEndTime = System.currentTimeMillis()
                    val elapsed = twoFingerEndTime - twoFingerDownTime

                    if (!twoFingerMoved && elapsed < TWO_FINGER_TAP_MS) {
                        client.rightClick(cursorX, cursorY)
                        triggerClick(true)
                        longPressFired = true
                    }

                    // Оставшийся палец НЕ подхватывает курсор — жест уже помечен
                    // как мультитач (gestureConsumed), одиночные действия ждут
                    // полного отрыва. Так после скролла нет фантомного клика.
                    twoFinger = false
                    scrollAccumX = 0f; scrollAccumY = 0f
                    invalidate()
                }
            }

            MotionEvent.ACTION_UP -> {
                when {
                    threeFinger -> { /* навигация уже сработала на move */ }
                    // Свежий след после 3-пальцевого жеста — не кликаем.
                    System.currentTimeMillis() - threeEndTime < 300L -> {}
                    dragSelectMode -> {
                        client.touch(cursorX, cursorY, 2)
                        dragSelectMode = false
                    }
                    // Тап-клик только для чистого одиночного касания: без
                    // мультитача в этом жесте (иначе фантомный клик после скролла).
                    !twoFinger && !longPressFired && !gestureConsumed -> {
                        val dist = hypot(event.x - downX, event.y - downY)
                        if (dist < tapSlop) {
                            client.touch(cursorX, cursorY, 0)
                            client.touch(cursorX, cursorY, 2)
                            triggerClick(false)
                        }
                    }
                }
                twoFinger = false
                threeFinger = false
                longPressFired = false
                gestureConsumed = false
                activeTouches.clear()
                invalidate()
            }

            MotionEvent.ACTION_CANCEL -> {
                if (dragSelectMode) {
                    client.touch(cursorX, cursorY, 2)
                    dragSelectMode = false
                }
                twoFinger = false
                threeFinger = false
                longPressFired = false
                gestureConsumed = false
                twoFingerEndTime = System.currentTimeMillis()
                scrollAccumX = 0f; scrollAccumY = 0f
                activeTouches.clear()
                invalidate()
            }
        }
        return true
    }

    /** Синхронизирует карту активных пальцев с текущим событием (для следов). */
    private fun syncActiveTouches(event: MotionEvent) {
        when (event.actionMasked) {
            MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> activeTouches.clear()
            else -> {
                val present = HashSet<Int>()
                for (i in 0 until event.pointerCount) {
                    val id = event.getPointerId(i)
                    present.add(id)
                    val p = activeTouches.getOrPut(id) { PointF() }
                    p.set(event.getX(i), event.getY(i))
                }
                // Палец, поднятый в POINTER_UP, ещё числится в событии — уберём его.
                if (event.actionMasked == MotionEvent.ACTION_POINTER_UP) {
                    activeTouches.remove(event.getPointerId(event.actionIndex))
                }
                activeTouches.keys.retainAll(present)
            }
        }
        ensureTrailAnim()
    }

    // ── 3 пальца: горизонтальный свайп → назад / вперёд ───────────────────────
    private fun handleThreeFingerMove(event: MotionEvent) {
        if (threeFired) return
        val midX = (0 until 3).sumOf { event.getX(it).toDouble() }.toFloat() / 3f
        val dx = midX - threeDownX
        if (abs(dx) > threeFingerNavThreshold) {
            val forward = dx > 0
            client.navigate(forward)
            showNavBadge(forward)
            performHapticFeedback(HapticFeedbackConstants.LONG_PRESS)
            threeFired = true
        }
    }

    // ── 1-палец: движение с sub-pixel аккумулятором и shake-to-find ──────────

    private fun handleOneFingerMove(event: MotionEvent) {
        val now = System.nanoTime()
        val rawDx = event.x - prevX
        val rawDy = event.y - prevY
        prevX = event.x; prevY = event.y
        lastMoveMs = SystemClock.elapsedRealtime()

        // dt между событиями (мс), ограничиваем — большие паузы не должны
        // рождать всплеск «скорости».
        val dtMs = if (lastMoveTimeNs == 0L) 8f
            else ((now - lastMoveTimeNs) / 1_000_000f).coerceIn(1f, 50f)
        lastMoveTimeNs = now

        // Мгновенная скорость пальца (px/мс) и её низкочастотное сглаживание —
        // стабилизирует множитель ускорения, чтоб он не дрожал.
        val speed = hypot(rawDx, rawDy) / dtMs
        smoothedSpeed += (speed - smoothedSpeed) * 0.5f

        // Нормируем скорость и берём smoothstep-долю для плавной кривой.
        val t = (smoothedSpeed / ACCEL_SPEED_REF).coerceIn(0f, 1f)
        val s = t * t * (3f - 2f * t)

        // Адаптивное сглаживание (антиджиттер): медленно → сильнее сглаживаем
        // (alpha мал), быстро → почти пропускаем как есть (alpha→1, без лага).
        val alpha = (0.35f + 0.65f * s).coerceIn(0.35f, 1f)
        smoothDx += (rawDx - smoothDx) * alpha
        smoothDy += (rawDy - smoothDy) * alpha

        // Множитель ускорения по сглаженной скорости.
        val gain = if (accelEnabled) ACCEL_MIN_GAIN + (ACCEL_MAX_GAIN - ACCEL_MIN_GAIN) * s
                   else 1f

        pendingDx += smoothDx * sensitivity * gain
        pendingDy += smoothDy * sensitivity * gain
        scheduleFrame()
    }

    /** Планирует отправку накопленного смещения на ближайший кадр (VSync). */
    private fun scheduleFrame() {
        if (!frameScheduled) {
            frameScheduled = true
            choreographer.postFrameCallback(frameCallback)
        }
    }

    /** Раз за кадр: сливаем целочисленное смещение, дробь оставляем на потом. */
    private fun flushMotion() {
        frameScheduled = false
        val dx = pendingDx.toInt()
        val dy = pendingDy.toInt()
        if (dx == 0 && dy == 0) return
        pendingDx -= dx
        pendingDy -= dy
        cursorX = clampX(cursorX + dx)
        cursorY = clampY(cursorY + dy)
        client.touch(cursorX, cursorY, 1)
        invalidate()
    }

    /** Сброс состояния движка при новом касании — без «скачка» на первом move. */
    private fun resetMotionEngine() {
        lastMoveTimeNs = 0L
        smoothedSpeed = 0f
        smoothDx = 0f; smoothDy = 0f
        pendingDx = 0f; pendingDy = 0f
    }

    fun setAccelEnabled(enabled: Boolean) { accelEnabled = enabled }

    override fun onDetachedFromWindow() {
        super.onDetachedFromWindow()
        choreographer.removeFrameCallback(frameCallback)
        handler.removeCallbacksAndMessages(null)
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
