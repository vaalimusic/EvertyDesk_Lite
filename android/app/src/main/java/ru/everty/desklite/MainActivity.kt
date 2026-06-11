package ru.everty.desklite

import android.app.Activity
import android.graphics.Color
import android.graphics.Typeface
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.text.InputType
import android.view.Gravity
import android.view.View
import android.view.ViewGroup
import android.view.inputmethod.InputMethodManager
import android.widget.Button
import android.widget.EditText
import android.widget.FrameLayout
import android.widget.LinearLayout
import android.widget.TextView

class MainActivity : Activity() {
    private val client = NativeClient()
    private lateinit var root: FrameLayout
    private var remoteView: RemoteView? = null
    private val handler = Handler(Looper.getMainLooper())

    private val brandBg    = Color.rgb(0x0E, 0x1B, 0x2B)
    private val brandGreen = Color.rgb(0x12, 0xC9, 0x72)
    private val cardBg     = Color.rgb(0x16, 0x24, 0x36)
    private val toolbarBg  = Color.argb(0xE0, 0x08, 0x12, 0x1E)

    // Флаг «правый клик активен»: следующий тап отправит правый клик
    private var rightClickPending = false
    private var rightClickBtn: Button? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        root = FrameLayout(this).apply { setBackgroundColor(brandBg) }
        setContentView(root)
        showConnectScreen()
    }

    // ── Экран подключения ─────────────────────────────────────────────────────
    private fun showConnectScreen() {
        root.removeAllViews()
        rightClickPending = false

        val col = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            gravity = Gravity.CENTER
            setPadding(dp(28), dp(28), dp(28), dp(28))
            layoutParams = FrameLayout.LayoutParams(MATCH, MATCH)
        }

        val title = TextView(this).apply {
            text = "EvertyDesk Lite"
            setTextColor(Color.WHITE)
            textSize = 26f
            typeface = Typeface.DEFAULT_BOLD
            gravity = Gravity.CENTER
        }
        val subtitle = TextView(this).apply {
            text = "Быстрый защищённый удалённый доступ"
            setTextColor(Color.rgb(0xA7, 0xC9, 0xBE))
            textSize = 13f
            gravity = Gravity.CENTER
            setPadding(0, dp(4), 0, dp(28))
        }

        val idInput = editField("ID партнёра", InputType.TYPE_CLASS_NUMBER)
        val pwInput = editField(
            "Пароль (необязательно)",
            InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_PASSWORD
        )

        val statusLabel = TextView(this).apply {
            text = ""
            setTextColor(Color.rgb(0xFF, 0x80, 0x80))
            textSize = 13f
            gravity = Gravity.CENTER
            setPadding(0, dp(12), 0, 0)
        }

        val connectBtn = Button(this).apply {
            text = "Подключиться"
            setTextColor(Color.WHITE)
            setBackgroundColor(brandGreen)
            textSize = 17f
            setOnClickListener {
                val id = idInput.text.toString().filter { it.isDigit() }
                if (id.isEmpty()) { statusLabel.text = "Введите ID"; return@setOnClickListener }
                statusLabel.text = "Подключение…"
                statusLabel.setTextColor(Color.rgb(0xA7, 0xC9, 0xBE))
                connect(id, pwInput.text.toString(), statusLabel)
            }
        }

        col.addView(title)
        col.addView(subtitle)
        col.addView(idInput, lpW())
        col.addView(spacer(dp(12)))
        col.addView(pwInput, lpW())
        col.addView(spacer(dp(20)))
        col.addView(connectBtn, lpWH(dp(52)))
        col.addView(statusLabel)
        root.addView(col)
    }

    // ── Подключение ───────────────────────────────────────────────────────────
    private fun connect(id: String, password: String, statusLabel: TextView) {
        if (!client.start(id, password)) {
            statusLabel.text = "Не удалось запустить сессию"
            statusLabel.setTextColor(Color.rgb(0xFF, 0x80, 0x80))
            return
        }
        showRemoteScreen()
    }

    // ── Удалённый экран ───────────────────────────────────────────────────────
    private fun showRemoteScreen() {
        root.removeAllViews()

        val rv = RemoteView(this, client).apply {
            layoutParams = FrameLayout.LayoutParams(MATCH, MATCH)
        }
        remoteView = rv
        root.addView(rv)

        // ── Статус-оверлей (сверху слева) ────────────────────────────────────
        val overlay = TextView(this).apply {
            setTextColor(Color.WHITE)
            setBackgroundColor(Color.argb(0xAA, 0, 0, 0))
            textSize = 11f
            setPadding(dp(10), dp(6), dp(10), dp(6))
            layoutParams = FrameLayout.LayoutParams(WRAP, WRAP, Gravity.TOP or Gravity.START)
        }
        root.addView(overlay)

        // ── Тулбар снизу ─────────────────────────────────────────────────────
        val toolbar = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            setBackgroundColor(toolbarBg)
            setPadding(dp(8), dp(4), dp(8), dp(4))
            layoutParams = FrameLayout.LayoutParams(MATCH, WRAP, Gravity.BOTTOM)
        }

        // Правый клик — тогглится; следующий тап будет правым кликом
        val rcBtn = toolbarButton("⊡ ПКМ", Color.rgb(0x55, 0x55, 0x55))
        rightClickBtn = rcBtn
        rcBtn.setOnClickListener {
            rightClickPending = !rightClickPending
            updateRightClickButton()
        }

        // Клавиатура
        val kbBtn = toolbarButton("⌨", Color.rgb(0x33, 0x44, 0x55))
        kbBtn.setOnClickListener { toggleKeyboard() }

        // Сброс зума
        val zoomBtn = toolbarButton("⊞ 1:1", Color.rgb(0x22, 0x44, 0x33))
        zoomBtn.setOnClickListener { rv.resetZoom() }

        // Отключить
        val discBtn = toolbarButton("✕ Выход", Color.rgb(0x77, 0x22, 0x22))
        discBtn.setOnClickListener { disconnect() }

        toolbar.addView(rcBtn,  lpToolbar())
        toolbar.addView(spacer(dp(4)))
        toolbar.addView(kbBtn,  lpToolbar())
        toolbar.addView(spacer(dp(4)))
        toolbar.addView(zoomBtn, lpToolbar())
        toolbar.addView(spacer(dp(4)))
        toolbar.addView(discBtn, lpToolbar())
        root.addView(toolbar)

        // Перехватываем тап с флагом правого клика
        rv.setRightClickCallback { x, y ->
            if (rightClickPending) {
                client.rightClick(x, y)
                rightClickPending = false
                updateRightClickButton()
                true
            } else false
        }

        rv.startRendering()

        // Обновляем статус каждые 500 мс
        val statusTick = object : Runnable {
            override fun run() {
                overlay.text = if (client.isConnected()) "● ${client.status()}" else client.status()
                handler.postDelayed(this, 500)
            }
        }
        handler.post(statusTick)
    }

    private fun updateRightClickButton() {
        rightClickBtn?.setBackgroundColor(
            if (rightClickPending) Color.rgb(0xC0, 0x50, 0x10)
            else Color.rgb(0x55, 0x55, 0x55)
        )
    }

    private fun toggleKeyboard() {
        val imm = getSystemService(INPUT_METHOD_SERVICE) as InputMethodManager
        val focused = currentFocus
        if (focused != null) {
            imm.toggleSoftInput(InputMethodManager.SHOW_FORCED, 0)
        } else {
            // Создаём скрытый EditText для фокуса клавиатуры
            val ghost = EditText(this).apply {
                setBackgroundColor(Color.TRANSPARENT)
                layoutParams = FrameLayout.LayoutParams(1, 1)
            }
            root.addView(ghost)
            ghost.requestFocus()
            imm.showSoftInput(ghost, InputMethodManager.SHOW_IMPLICIT)
        }
    }

    private fun disconnect() {
        remoteView?.stopRendering()
        client.stop()
        handler.removeCallbacksAndMessages(null)
        rightClickPending = false
        rightClickBtn = null
        showConnectScreen()
    }

    override fun onDestroy() {
        remoteView?.stopRendering()
        client.stop()
        super.onDestroy()
    }

    // ── helpers ───────────────────────────────────────────────────────────────
    private fun dp(v: Int) = (v * resources.displayMetrics.density).toInt()
    private fun editField(hint: String, inputType: Int) = EditText(this).apply {
        this.hint = hint
        this.inputType = inputType
        setTextColor(Color.WHITE)
        setHintTextColor(Color.rgb(0x6A, 0x77, 0x86))
        setBackgroundColor(cardBg)
        setPadding(dp(16), dp(14), dp(16), dp(14))
        textSize = 16f
    }
    private fun toolbarButton(text: String, bg: Int) = Button(this).apply {
        this.text = text
        setTextColor(Color.WHITE)
        setBackgroundColor(bg)
        textSize = 12f
        setPadding(dp(8), dp(4), dp(8), dp(4))
    }
    private fun spacer(size: Int) = View(this).apply {
        layoutParams = LinearLayout.LayoutParams(size, WRAP)
    }
    private fun lpW()           = LinearLayout.LayoutParams(MATCH, WRAP)
    private fun lpWH(h: Int)    = LinearLayout.LayoutParams(MATCH, h)
    private fun lpToolbar()     = LinearLayout.LayoutParams(0, WRAP, 1f)

    companion object {
        const val MATCH = ViewGroup.LayoutParams.MATCH_PARENT
        const val WRAP  = ViewGroup.LayoutParams.WRAP_CONTENT
    }
}
