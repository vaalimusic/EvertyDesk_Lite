package ru.everty.desklite

import android.app.Activity
import android.graphics.Color
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.text.InputType
import android.view.Gravity
import android.view.View
import android.view.ViewGroup
import android.widget.Button
import android.widget.EditText
import android.widget.FrameLayout
import android.widget.LinearLayout
import android.widget.TextView

/**
 * Главный экран EvertyDesk Android (только исходящие подключения).
 *
 * Два состояния:
 *   • Connect — ввод ID + пароля, кнопка «Подключиться»
 *   • Remote  — RemoteView на весь экран + статус-оверлей
 *
 * UI собран кодом (без XML) для простоты скелета — потом можно перенести в
 * Compose/XML по вкусу.
 */
class MainActivity : Activity() {
    private val client = NativeClient()
    private lateinit var root: FrameLayout
    private var remoteView: RemoteView? = null
    private lateinit var statusLabel: TextView
    private val handler = Handler(Looper.getMainLooper())

    private val brandBg = Color.rgb(0x0E, 0x1B, 0x2B)
    private val brandGreen = Color.rgb(0x12, 0xC9, 0x72)
    private val cardBg = Color.rgb(0x16, 0x24, 0x36)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        root = FrameLayout(this).apply { setBackgroundColor(brandBg) }
        setContentView(root)
        showConnectScreen()
    }

    // ── Экран подключения ─────────────────────────────────────────────────────
    private fun showConnectScreen() {
        root.removeAllViews()

        val col = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            gravity = Gravity.CENTER
            setPadding(dp(28), dp(28), dp(28), dp(28))
            layoutParams = FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.MATCH_PARENT
            )
        }

        val title = TextView(this).apply {
            text = "EvertyDesk Lite"
            setTextColor(Color.WHITE)
            textSize = 26f
            gravity = Gravity.CENTER
        }
        val subtitle = TextView(this).apply {
            text = "Быстрый защищённый удалённый доступ"
            setTextColor(Color.rgb(0xA7, 0xC9, 0xBE))
            textSize = 13f
            gravity = Gravity.CENTER
            setPadding(0, dp(4), 0, dp(28))
        }

        val idInput = EditText(this).apply {
            hint = "ID партнёра"
            inputType = InputType.TYPE_CLASS_NUMBER
            setTextColor(Color.WHITE)
            setHintTextColor(Color.rgb(0x6A, 0x77, 0x86))
            setBackgroundColor(cardBg)
            setPadding(dp(16), dp(14), dp(16), dp(14))
            textSize = 18f
        }
        val pwInput = EditText(this).apply {
            hint = "Пароль (необязательно)"
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_PASSWORD
            setTextColor(Color.WHITE)
            setHintTextColor(Color.rgb(0x6A, 0x77, 0x86))
            setBackgroundColor(cardBg)
            setPadding(dp(16), dp(14), dp(16), dp(14))
            textSize = 16f
        }

        statusLabel = TextView(this).apply {
            text = ""
            setTextColor(Color.rgb(0xA7, 0xC9, 0xBE))
            textSize = 13f
            gravity = Gravity.CENTER
            setPadding(0, dp(14), 0, 0)
        }

        val connectBtn = Button(this).apply {
            text = "Подключиться"
            setTextColor(Color.WHITE)
            setBackgroundColor(brandGreen)
            textSize = 17f
            setOnClickListener {
                val id = idInput.text.toString().filter { it.isDigit() }
                if (id.isEmpty()) {
                    statusLabel.text = "Введите ID"
                    return@setOnClickListener
                }
                connect(id, pwInput.text.toString())
            }
        }

        col.addView(title)
        col.addView(subtitle)
        col.addView(idInput, lp())
        col.addView(spacer(dp(12)))
        col.addView(pwInput, lp())
        col.addView(spacer(dp(20)))
        col.addView(connectBtn, lpHeight(dp(52)))
        col.addView(statusLabel)
        root.addView(col)
    }

    // ── Подключение → переход на удалённый экран ──────────────────────────────
    private fun connect(id: String, password: String) {
        statusLabel.text = "Подключение…"
        if (!client.start(id, password)) {
            statusLabel.text = "Не удалось запустить сессию"
            return
        }
        showRemoteScreen()
    }

    private fun showRemoteScreen() {
        root.removeAllViews()
        val rv = RemoteView(this, client)
        rv.layoutParams = FrameLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT,
            ViewGroup.LayoutParams.MATCH_PARENT
        )
        remoteView = rv
        root.addView(rv)

        // Статус-оверлей сверху
        val overlay = TextView(this).apply {
            setTextColor(Color.WHITE)
            setBackgroundColor(Color.argb(0xB0, 0, 0, 0))
            textSize = 12f
            setPadding(dp(12), dp(8), dp(12), dp(8))
            layoutParams = FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
                Gravity.TOP or Gravity.START
            )
        }
        root.addView(overlay)

        // Кнопка отключения
        val disconnectBtn = Button(this).apply {
            text = "✕"
            setTextColor(Color.WHITE)
            setBackgroundColor(Color.argb(0xB0, 0xC0, 0x30, 0x30))
            layoutParams = FrameLayout.LayoutParams(
                dp(44), dp(44), Gravity.TOP or Gravity.END
            )
            setOnClickListener { disconnect() }
        }
        root.addView(disconnectBtn)

        rv.startRendering()

        // Обновляем оверлей-статус
        val statusTick = object : Runnable {
            override fun run() {
                overlay.text = client.status()
                if (client.isConnected()) {
                    overlay.text = "● ${client.status()}"
                }
                handler.postDelayed(this, 500)
            }
        }
        handler.post(statusTick)
    }

    private fun disconnect() {
        remoteView?.stopRendering()
        client.stop()
        handler.removeCallbacksAndMessages(null)
        showConnectScreen()
    }

    override fun onDestroy() {
        remoteView?.stopRendering()
        client.stop()
        super.onDestroy()
    }

    // ── helpers ───────────────────────────────────────────────────────────────
    private fun dp(v: Int): Int = (v * resources.displayMetrics.density).toInt()
    private fun lp() = LinearLayout.LayoutParams(
        ViewGroup.LayoutParams.MATCH_PARENT,
        ViewGroup.LayoutParams.WRAP_CONTENT
    )
    private fun lpHeight(h: Int) = LinearLayout.LayoutParams(
        ViewGroup.LayoutParams.MATCH_PARENT, h
    )
    private fun spacer(h: Int) = View(this).apply {
        layoutParams = LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, h)
    }
}
