package ru.everty.desklite

import android.accessibilityservice.AccessibilityService
import android.accessibilityservice.GestureDescription
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.graphics.Path
import android.graphics.PixelFormat
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.view.Gravity
import android.view.View
import android.view.WindowManager
import android.view.accessibility.AccessibilityEvent
import kotlin.math.hypot

/**
 * Служба доступности — «хост» EvertyDesk на Android (тачпад-режим).
 *
 * Rust-хост (android_ffi) принимает Mouse/KeyEvent от подключённого клиента и
 * через JNI вызывает статические методы onMouseNative/onKeyNative. Здесь мы:
 *   • держим оверлей-курсор (TYPE_ACCESSIBILITY_OVERLAY — без отдельного
 *     разрешения на «поверх других приложений»);
 *   • превращаем поток событий в жесты: тап, ПКМ (долгое нажатие), скролл,
 *     перетаскивание, а управляющие клавиши — в глобальные действия (Назад/Домой).
 *
 * Видео не захватывается: оператор смотрит прямо на экран устройства (ТВ),
 * телефон работает слепым трекпадом к большому экрану.
 */
class EvertyInputService : AccessibilityService() {

    companion object {
        @Volatile
        private var instance: EvertyInputService? = null

        /** true если служба подключена (доступность включена). */
        @JvmStatic
        fun isRunning(): Boolean = instance != null

        // ── Вызовы из Rust (JNI). Приходят на потоке хоста → маршалим в main. ──
        @JvmStatic
        fun onMouseNative(kind: Int, button: Int, x: Int, y: Int) {
            instance?.postMouse(kind, button, x, y)
        }

        @JvmStatic
        fun onKeyNative(code: Int, alt: Boolean) {
            instance?.postKey(code, alt)
        }

        /**
         * Хост запущен/остановлен: показываем/прячем курсор и уведомление в шторке.
         * Вызывается из UI (MainActivity) при старте/стопе хост-режима.
         */
        @JvmStatic
        fun setHostActive(active: Boolean) {
            instance?.applyHostActive(active)
        }

        // ControlKey (см. rustdesk_proto::ControlKey)
        private const val CK_ESCAPE = 8
        private const val CK_HOME = 21
        private const val CK_LEFT = 22
        private const val CK_RIGHT = 28

        private const val CHANNEL_ID = "everty_host"
        private const val NOTIF_ID = 1001
    }

    private val main = Handler(Looper.getMainLooper())
    private var wm: WindowManager? = null
    private var cursor: CursorView? = null
    private var lp: WindowManager.LayoutParams? = null

    private var screenW = 0
    private var screenH = 0
    private var cx = 0
    private var cy = 0

    // Отслеживание нажатия ЛКМ для различения тап / перетаскивание.
    private var leftDown = false
    private var downX = 0
    private var downY = 0
    private var downTime = 0L

    private var hostActive = false

    override fun onServiceConnected() {
        super.onServiceConnected()
        instance = this
        wm = getSystemService(WINDOW_SERVICE) as WindowManager
        createNotificationChannel()
        addCursor()
        // Курсор виден только когда хост запущен — по подключению службы прячем.
        applyHostActive(hostActive)
    }

    override fun onAccessibilityEvent(event: AccessibilityEvent?) {}
    override fun onInterrupt() {}

    override fun onUnbind(intent: Intent?): Boolean {
        cancelNotification()
        removeCursor()
        instance = null
        return super.onUnbind(intent)
    }

    override fun onDestroy() {
        cancelNotification()
        removeCursor()
        instance = null
        super.onDestroy()
    }

    // ── Активность хоста: курсор + уведомление ────────────────────────────────

    fun applyHostActive(active: Boolean) {
        hostActive = active
        main.post {
            cursor?.visibility = if (active) View.VISIBLE else View.GONE
            if (active) {
                // Ставим курсор в центр при запуске.
                moveCursorTo(screenW / 2, screenH / 2)
                showNotification()
            } else {
                cancelNotification()
            }
        }
    }

    private fun createNotificationChannel() {
        val nm = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        val ch = NotificationChannel(
            CHANNEL_ID,
            "Хост-режим EvertyDesk",
            NotificationManager.IMPORTANCE_LOW,
        ).apply {
            description = "Показывает, что устройством можно управлять удалённо"
            setShowBadge(false)
        }
        nm.createNotificationChannel(ch)
    }

    private fun showNotification() {
        val open = PendingIntent.getActivity(
            this, 0,
            Intent(this, MainActivity::class.java)
                .addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val notif = Notification.Builder(this, CHANNEL_ID)
            .setSmallIcon(R.drawable.edesk_lite_logo)
            .setContentTitle("Хост активен — управление с телефона")
            .setContentText("Только трекпад · изображение и звук не передаются")
            .setOngoing(true)
            .setContentIntent(open)
            .build()
        val nm = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        runCatching { nm.notify(NOTIF_ID, notif) }
    }

    private fun cancelNotification() {
        val nm = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        runCatching { nm.cancel(NOTIF_ID) }
    }

    // ── Оверлей-курсор ────────────────────────────────────────────────────────

    private fun addCursor() {
        val dm = resources.displayMetrics
        screenW = dm.widthPixels
        screenH = dm.heightPixels
        cx = screenW / 2
        cy = screenH / 2
        val size = (30 * dm.density).toInt()
        val v = CursorView(this)
        val p = WindowManager.LayoutParams(
            size, size,
            WindowManager.LayoutParams.TYPE_ACCESSIBILITY_OVERLAY,
            WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE or
                WindowManager.LayoutParams.FLAG_NOT_TOUCHABLE or
                WindowManager.LayoutParams.FLAG_LAYOUT_IN_SCREEN or
                WindowManager.LayoutParams.FLAG_LAYOUT_NO_LIMITS,
            PixelFormat.TRANSLUCENT,
        )
        p.gravity = Gravity.TOP or Gravity.START
        p.x = cx - size / 2
        p.y = cy - size / 2
        v.visibility = if (hostActive) View.VISIBLE else View.GONE
        runCatching { wm?.addView(v, p) }
        cursor = v
        lp = p
    }

    private fun removeCursor() {
        cursor?.let { v -> runCatching { wm?.removeView(v) } }
        cursor = null
        lp = null
    }

    private fun moveCursorTo(x: Int, y: Int) {
        cx = x.coerceIn(0, maxOf(screenW - 1, 0))
        cy = y.coerceIn(0, maxOf(screenH - 1, 0))
        val p = lp ?: return
        val v = cursor ?: return
        p.x = cx - p.width / 2
        p.y = cy - p.height / 2
        runCatching { wm?.updateViewLayout(v, p) }
    }

    // ── Маршалинг событий в main-поток ────────────────────────────────────────

    fun postMouse(kind: Int, button: Int, x: Int, y: Int) {
        main.post { handleMouse(kind, button, x, y) }
    }

    fun postKey(code: Int, alt: Boolean) {
        main.post { handleKey(code, alt) }
    }

    // ── Обработка мыши ────────────────────────────────────────────────────────
    // kind: 0=move 1=down 2=up 3=wheel | button: 1=left 2=right 4=wheel
    private fun handleMouse(kind: Int, button: Int, x: Int, y: Int) {
        when (kind) {
            0 -> moveCursorTo(x, y)
            1 -> {
                if (button == 1) {
                    leftDown = true
                    downX = x; downY = y
                    downTime = SystemClock.uptimeMillis()
                }
                moveCursorTo(x, y)
            }
            2 -> {
                if (button == 1 && leftDown) {
                    leftDown = false
                    moveCursorTo(x, y)
                    val dt = SystemClock.uptimeMillis() - downTime
                    val dist = hypot((x - downX).toFloat(), (y - downY).toFloat())
                    val slop = 24f * resources.displayMetrics.density
                    if (dist < slop && dt < 600) {
                        tapAt(x, y)
                    } else {
                        dragAt(downX, downY, x, y)
                    }
                } else if (button == 2) {
                    moveCursorTo(x, y)
                    longPressAt(x, y) // ПКМ → контекстное меню
                }
            }
            3 -> scrollBy(cx, cy, y) // wheel: y = вертикальная дельта
        }
    }

    // ── Обработка клавиш → глобальные действия ────────────────────────────────
    private fun handleKey(code: Int, alt: Boolean) {
        when {
            code == CK_ESCAPE -> performGlobalAction(GLOBAL_ACTION_BACK)
            code == CK_HOME -> performGlobalAction(GLOBAL_ACTION_HOME)
            alt && code == CK_LEFT -> performGlobalAction(GLOBAL_ACTION_BACK)     // 3 пальца ←
            alt && code == CK_RIGHT -> performGlobalAction(GLOBAL_ACTION_RECENTS) // 3 пальца →
        }
    }

    // ── Жесты ─────────────────────────────────────────────────────────────────

    private fun clampX(v: Int) = v.coerceIn(0, maxOf(screenW - 1, 0)).toFloat()
    private fun clampY(v: Int) = v.coerceIn(0, maxOf(screenH - 1, 0)).toFloat()

    private fun dispatch(path: Path, startMs: Long, durationMs: Long) {
        val stroke = GestureDescription.StrokeDescription(path, startMs, durationMs)
        val gesture = GestureDescription.Builder().addStroke(stroke).build()
        runCatching { dispatchGesture(gesture, null, null) }
    }

    private fun tapAt(x: Int, y: Int) {
        val p = Path().apply { moveTo(clampX(x), clampY(y)) }
        dispatch(p, 0, 40)
    }

    private fun longPressAt(x: Int, y: Int) {
        val p = Path().apply { moveTo(clampX(x), clampY(y)) }
        dispatch(p, 0, 650) // > longPressTimeout → контекстное меню
    }

    private fun dragAt(x0: Int, y0: Int, x1: Int, y1: Int) {
        val p = Path().apply {
            moveTo(clampX(x0), clampY(y0))
            lineTo(clampX(x1), clampY(y1))
        }
        dispatch(p, 0, 320)
    }

    private fun scrollBy(x: Int, y: Int, delta: Int) {
        if (delta == 0) return
        // Дельта колеса → вертикальный свайп. Масштаб подобран под «шаги» тачпада.
        val amount = (delta * 34f * resources.displayMetrics.density)
        val startY = clampY(y)
        // Свайпаем в противоположную дельте сторону (палец ведёт контент).
        val endY = (startY - amount).coerceIn(0f, maxOf(screenH - 1, 0).toFloat())
        val p = Path().apply {
            moveTo(clampX(x), startY)
            lineTo(clampX(x), endY)
        }
        dispatch(p, 0, 120)
    }

    // ── Вид курсора — стрелка-указатель ───────────────────────────────────────
    private class CursorView(ctx: android.content.Context) : View(ctx) {
        private val fill = Paint(Paint.ANTI_ALIAS_FLAG).apply {
            color = Color.rgb(0x12, 0xC9, 0x72)
            style = Paint.Style.FILL
        }
        private val stroke = Paint(Paint.ANTI_ALIAS_FLAG).apply {
            color = Color.WHITE
            style = Paint.Style.STROKE
            strokeWidth = 2f * resources.displayMetrics.density
        }
        private val shadow = Paint(Paint.ANTI_ALIAS_FLAG).apply {
            color = Color.argb(0x66, 0, 0, 0)
            style = Paint.Style.FILL
        }

        override fun onDraw(canvas: Canvas) {
            val w = width.toFloat()
            val h = height.toFloat()
            // Классическая стрелка курсора из левого-верхнего угла.
            val p = Path().apply {
                moveTo(w * 0.12f, h * 0.08f)
                lineTo(w * 0.12f, h * 0.86f)
                lineTo(w * 0.34f, h * 0.64f)
                lineTo(w * 0.50f, h * 0.96f)
                lineTo(w * 0.64f, h * 0.90f)
                lineTo(w * 0.48f, h * 0.58f)
                lineTo(w * 0.78f, h * 0.56f)
                close()
            }
            // лёгкая тень для читаемости на любом фоне
            canvas.save()
            canvas.translate(1.5f * resources.displayMetrics.density, 1.5f * resources.displayMetrics.density)
            canvas.drawPath(p, shadow)
            canvas.restore()
            canvas.drawPath(p, fill)
            canvas.drawPath(p, stroke)
        }
    }
}
