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
import android.view.ViewGroup.LayoutParams.MATCH_PARENT
import android.view.ViewGroup.LayoutParams.WRAP_CONTENT
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
    private val toolbarBg  = Color.argb(0xE8, 0x08, 0x12, 0x1E)

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
        rightClickBtn = null

        val col = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            gravity = Gravity.CENTER_HORIZONTAL
            setPadding(dp(32), 0, dp(32), 0)
            layoutParams = FrameLayout.LayoutParams(MATCH_PARENT, MATCH_PARENT, Gravity.CENTER)
        }

        col.addView(TextView(this).apply {
            text = "EvertyDesk"
            setTextColor(brandGreen)
            textSize = 32f
            typeface = Typeface.DEFAULT_BOLD
            gravity = Gravity.CENTER
        }, matchWrap())

        col.addView(vSpace(dp(4)))

        col.addView(TextView(this).apply {
            text = "Lite"
            setTextColor(Color.WHITE)
            textSize = 18f
            gravity = Gravity.CENTER
        }, matchWrap())

        col.addView(vSpace(dp(8)))

        col.addView(TextView(this).apply {
            text = "Быстрый защищённый удалённый доступ"
            setTextColor(Color.rgb(0x8A, 0xA9, 0x9E))
            textSize = 13f
            gravity = Gravity.CENTER
        }, matchWrap())

        col.addView(vSpace(dp(36)))

        val idInput = EditText(this).apply {
            hint = "ID партнёра"
            inputType = InputType.TYPE_CLASS_NUMBER
            setTextColor(Color.WHITE)
            setHintTextColor(Color.rgb(0x55, 0x66, 0x77))
            setBackgroundColor(cardBg)
            setPadding(dp(16), dp(14), dp(16), dp(14))
            textSize = 18f
        }
        col.addView(idInput, matchWrap())

        col.addView(vSpace(dp(12)))

        val pwInput = EditText(this).apply {
            hint = "Пароль (необязательно)"
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_PASSWORD
            setTextColor(Color.WHITE)
            setHintTextColor(Color.rgb(0x55, 0x66, 0x77))
            setBackgroundColor(cardBg)
            setPadding(dp(16), dp(14), dp(16), dp(14))
            textSize = 16f
        }
        col.addView(pwInput, matchWrap())

        col.addView(vSpace(dp(24)))

        val statusLabel = TextView(this).apply {
            text = " "
            setTextColor(Color.rgb(0xA7, 0xC9, 0xBE))
            textSize = 13f
            gravity = Gravity.CENTER
        }

        val connectBtn = Button(this).apply {
            text = "Подключиться"
            setTextColor(Color.WHITE)
            setBackgroundColor(brandGreen)
            textSize = 17f
            setOnClickListener {
                val id = idInput.text.toString().filter { it.isDigit() }
                if (id.isEmpty()) {
                    statusLabel.text = "Введите ID партнёра"
                    statusLabel.setTextColor(Color.rgb(0xFF, 0x88, 0x66))
                    return@setOnClickListener
                }
                statusLabel.text = "Подключение…"
                statusLabel.setTextColor(Color.rgb(0xA7, 0xC9, 0xBE))
                connect(id, pwInput.text.toString(), statusLabel)
            }
        }
        col.addView(connectBtn, LinearLayout.LayoutParams(MATCH_PARENT, dp(52)))
        col.addView(vSpace(dp(12)))
        col.addView(statusLabel, matchWrap())

        root.addView(col)
    }

    // ── Подключение ───────────────────────────────────────────────────────────
    private fun connect(id: String, password: String, statusLabel: TextView) {
        if (!client.start(id, password)) {
            statusLabel.text = "Не удалось запустить сессию"
            statusLabel.setTextColor(Color.rgb(0xFF, 0x88, 0x66))
            return
        }
        showRemoteScreen()
    }

    // ── Удалённый экран ───────────────────────────────────────────────────────
    private fun showRemoteScreen() {
        root.removeAllViews()

        val rv = RemoteView(this, client).apply {
            layoutParams = FrameLayout.LayoutParams(MATCH_PARENT, MATCH_PARENT)
        }
        remoteView = rv
        root.addView(rv)

        // Статус-оверлей — сверху слева
        val overlay = TextView(this).apply {
            setTextColor(Color.WHITE)
            setBackgroundColor(Color.argb(0xAA, 0, 0, 0))
            textSize = 11f
            setPadding(dp(10), dp(6), dp(10), dp(6))
        }
        root.addView(overlay, FrameLayout.LayoutParams(WRAP_CONTENT, WRAP_CONTENT,
            Gravity.TOP or Gravity.START))

        // Тулбар снизу
        val toolbar = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            setBackgroundColor(toolbarBg)
            gravity = Gravity.CENTER_VERTICAL
            setPadding(dp(6), dp(6), dp(6), dp(6))
        }

        val rcBtn = makeToolBtn("⊡  ПКМ", Color.rgb(0x44, 0x44, 0x55))
        rightClickBtn = rcBtn
        rcBtn.setOnClickListener {
            rightClickPending = !rightClickPending
            rcBtn.setBackgroundColor(
                if (rightClickPending) Color.rgb(0xCC, 0x55, 0x11)
                else Color.rgb(0x44, 0x44, 0x55)
            )
        }

        val kbBtn = makeToolBtn("⌨  Клав.", Color.rgb(0x22, 0x44, 0x55))
        kbBtn.setOnClickListener { toggleKeyboard(rv) }

        val zoomBtn = makeToolBtn("⊞  1:1", Color.rgb(0x22, 0x44, 0x33))
        zoomBtn.setOnClickListener { rv.resetZoom() }

        val discBtn = makeToolBtn("✕  Выход", Color.rgb(0x66, 0x22, 0x22))
        discBtn.setOnClickListener { disconnect() }

        val btnLp = LinearLayout.LayoutParams(0, dp(40), 1f).apply {
            setMargins(dp(3), 0, dp(3), 0)
        }
        toolbar.addView(rcBtn, btnLp)
        toolbar.addView(kbBtn, LinearLayout.LayoutParams(0, dp(40), 1f).also {
            it.setMargins(dp(3), 0, dp(3), 0) })
        toolbar.addView(zoomBtn, LinearLayout.LayoutParams(0, dp(40), 1f).also {
            it.setMargins(dp(3), 0, dp(3), 0) })
        toolbar.addView(discBtn, LinearLayout.LayoutParams(0, dp(40), 1f).also {
            it.setMargins(dp(3), 0, dp(3), 0) })

        root.addView(toolbar, FrameLayout.LayoutParams(MATCH_PARENT, WRAP_CONTENT,
            Gravity.BOTTOM))

        // callback: если ПКМ-режим активен — следующий тап = правый клик
        rv.setRightClickCallback { x, y ->
            if (rightClickPending) {
                client.rightClick(x, y)
                rightClickPending = false
                rcBtn.setBackgroundColor(Color.rgb(0x44, 0x44, 0x55))
                true
            } else false
        }

        rv.startRendering()

        val statusTick = object : Runnable {
            override fun run() {
                overlay.text = if (client.isConnected()) "● ${client.status()}" else client.status()
                handler.postDelayed(this, 500)
            }
        }
        handler.post(statusTick)
    }

    private fun disconnect() {
        remoteView?.stopRendering()
        client.stop()
        handler.removeCallbacksAndMessages(null)
        rightClickPending = false
        rightClickBtn = null
        remoteView = null
        showConnectScreen()
    }

    override fun onDestroy() {
        remoteView?.stopRendering()
        client.stop()
        super.onDestroy()
    }

    // ── helpers ───────────────────────────────────────────────────────────────
    private fun dp(v: Int) = (v * resources.displayMetrics.density).toInt()

    /** Вертикальный пробел для LinearLayout с ориентацией VERTICAL */
    private fun vSpace(h: Int) = View(this).apply {
        layoutParams = LinearLayout.LayoutParams(MATCH_PARENT, h)
    }

    private fun matchWrap() = LinearLayout.LayoutParams(MATCH_PARENT, WRAP_CONTENT)

    private fun makeToolBtn(text: String, bg: Int) = Button(this).apply {
        this.text = text
        setTextColor(Color.WHITE)
        setBackgroundColor(bg)
        textSize = 11f
        setPadding(dp(4), 0, dp(4), 0)
    }

    private fun toggleKeyboard(anchor: View) {
        anchor.requestFocus()
        val imm = getSystemService(INPUT_METHOD_SERVICE) as InputMethodManager
        imm.toggleSoftInput(InputMethodManager.SHOW_FORCED, 0)
    }
}
