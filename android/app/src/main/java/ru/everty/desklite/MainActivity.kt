package ru.everty.desklite

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.graphics.Color
import android.graphics.Typeface
import android.graphics.drawable.GradientDrawable
import android.net.Uri
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.text.Editable
import android.text.InputType
import android.text.TextWatcher
import android.view.Gravity
import android.view.KeyEvent
import android.view.View
import android.view.ViewGroup.LayoutParams.MATCH_PARENT
import android.view.ViewGroup.LayoutParams.WRAP_CONTENT
import android.view.inputmethod.InputMethodManager
import android.widget.Button
import android.widget.EditText
import android.widget.FrameLayout
import android.widget.HorizontalScrollView
import android.widget.ImageView
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import android.widget.Toast
import org.json.JSONArray
import org.json.JSONObject
import java.util.UUID
import kotlin.concurrent.thread

class MainActivity : Activity() {
    private val client = NativeClient()
    private lateinit var root: FrameLayout
    private var remoteView: RemoteView? = null
    private val handler = Handler(Looper.getMainLooper())

    private val brandBg    = Color.rgb(0xF5, 0xF7, 0xF4)
    private val brandGreen = Color.rgb(0x12, 0xC9, 0x72)
    private val cardBg     = Color.WHITE
    private val textMain   = Color.rgb(0x0D, 0x12, 0x10)
    private val textSoft   = Color.rgb(0x5F, 0x6C, 0x67)
    private val lineSoft   = Color.rgb(0xDE, 0xE7, 0xE2)
    private val blackInk   = Color.rgb(0x10, 0x12, 0x14)
    private val toolbarBg  = Color.argb(0xF0, 0x0D, 0x12, 0x10)

    private var rightClickPending = false
    private var rightClickBtn: Button? = null

    // Скрытый EditText-прокси для клавиатурного ввода
    private var keyProxy: EditText? = null
    private var kbPanel: View? = null
    private var kbVisible = false

    private val prefs by lazy { getSharedPreferences("everty_prefs", Context.MODE_PRIVATE) }
    private val PREF_LAST_ID = "last_id"
    private val PREF_API_URL = "api_url"
    private val PREF_ID_SERVER = "id_server"
    private val PREF_RELAY_SERVER = "relay_server"
    private val PREF_PUBLIC_KEY = "public_key"
    private val PREF_AB_ACCOUNT = "address_book_account"
    private val PREF_AB_TOKEN = "address_book_access_token"
    private val PREF_AB_GUID = "address_book_guid"
    private val PREF_AB_CONTACTS = "address_book_contacts"
    private val PREF_DEVICE_UUID = "device_uuid"
    private val PREF_LOCAL_ID = "local_id"
    private val PREF_RECENT_SESSIONS = "recent_sessions"

    private val defaultApiUrl = "https://desk.everty.ru"
    private val defaultIdServer = "edesk.server1.everty.ru"
    private val defaultRelayServer = "edesk.server1.everty.ru"
    private val defaultPublicKey = "MrGdbay3g8Qr84YYnxr4qLjw5zLWM1oAOdfehbBnlRs="

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        root = FrameLayout(this).apply { setBackgroundColor(brandBg) }
        setContentView(root)
        showConnectScreen()
    }

    // ── Экран подключения ─────────────────────────────────────────────────────
    private fun showConnectScreen() {
        rightClickPending = false
        rightClickBtn = null

        val col = showAppScreen(
            active = "connect",
            title = "EvertyDesk Lite",
            subtitle = "Быстрый защищённый удалённый доступ",
        )

        val idInput = makeInput("ID партнёра", false).apply {
            inputType = InputType.TYPE_CLASS_NUMBER
            textSize = 18f
            // Восстанавливаем последний ID
            prefs.getString(PREF_LAST_ID, "")?.takeIf { it.isNotEmpty() }?.let {
                setText(it)
                setSelection(it.length)
            }
        }
        col.addView(idInput, matchWrap())

        col.addView(vSpace(dp(12)))

        val pwInput = makeInput("Пароль (необязательно)", true)
        col.addView(pwInput, matchWrap())

        col.addView(vSpace(dp(24)))

        val statusLabel = TextView(this).apply {
            text = " "
            setTextColor(textSoft)
            textSize = 13f
            gravity = Gravity.CENTER
        }

        val connectBtn = makePrimaryButton("Подключиться") {
            val id = idInput.text.toString().filter { it.isDigit() }
            if (id.isEmpty()) {
                statusLabel.text = "Введите ID партнёра"
                statusLabel.setTextColor(Color.rgb(0xE3, 0x4B, 0x2F))
                return@makePrimaryButton
            }
            prefs.edit().putString(PREF_LAST_ID, id).apply()
            statusLabel.text = "Подключение..."
            statusLabel.setTextColor(textSoft)
            connect(id, pwInput.text.toString(), statusLabel)
        }.apply {
            textSize = 17f
        }
        col.addView(connectBtn, LinearLayout.LayoutParams(MATCH_PARENT, dp(52)))
        col.addView(vSpace(dp(12)))
        col.addView(statusLabel, matchWrap())

        val recent = loadRecentSessions()
        if (recent.isNotEmpty()) {
            col.addView(vSpace(dp(20)))
            col.addView(sectionHeader("Последние сеансы"))
            col.addView(vSpace(dp(8)))
            recent.forEach { remoteId ->
                col.addView(recentSessionCard(remoteId, idInput, pwInput, statusLabel))
                col.addView(vSpace(dp(8)))
            }
        }
    }

    private fun recentSessionCard(
        remoteId: String,
        idInput: EditText,
        pwInput: EditText,
        statusLabel: TextView,
    ): LinearLayout =
        LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            background = roundedBg(cardBg, 18, lineSoft)
            setPadding(dp(14), dp(10), dp(10), dp(10))

            addView(LinearLayout(this@MainActivity).apply {
                orientation = LinearLayout.VERTICAL
                addView(TextView(this@MainActivity).apply {
                    text = "ID $remoteId"
                    setTextColor(textMain)
                    textSize = 16f
                    typeface = Typeface.DEFAULT_BOLD
                }, matchWrap())
                addView(TextView(this@MainActivity).apply {
                    text = "Недавнее подключение"
                    setTextColor(textSoft)
                    textSize = 12f
                }, matchWrap())
            }, LinearLayout.LayoutParams(0, WRAP_CONTENT, 1f))

            addView(makeSecondaryButton("Открыть") {
                idInput.setText(remoteId)
                idInput.setSelection(idInput.text.length)
                connect(remoteId, pwInput.text.toString(), statusLabel)
            }, LinearLayout.LayoutParams(dp(104), dp(42)))
        }

    // ── Адресная книга ───────────────────────────────────────────────────────
    private fun showContactsScreen() {
        val col = showAppScreen(
            active = "contacts",
            title = "Адресная книга",
            subtitle = "Авторизация и контакты EvertyDesk",
        )

        val account = prefs.getString(PREF_AB_ACCOUNT, "").orEmpty()
        val token = prefs.getString(PREF_AB_TOKEN, "").orEmpty()
        val guid = prefs.getString(PREF_AB_GUID, "").orEmpty()
        val signedIn = token.isNotBlank() && guid.isNotBlank()

        val statusLabel = TextView(this).apply {
            text = if (signedIn) "Вход выполнен: $account" else "Войдите в аккаунт адресной книги"
            setTextColor(textSoft)
            textSize = 13f
            gravity = Gravity.CENTER
        }

        if (!signedIn) {
            val accountInput = makeInput("Логин или e-mail", false).apply {
                setText(account)
                setSelection(text.length)
            }
            val passwordInput = makeInput("Пароль или токен", true)

            col.addView(accountInput, matchWrap())
            col.addView(vSpace(dp(10)))
            col.addView(passwordInput, matchWrap())
            col.addView(vSpace(dp(14)))

            col.addView(makePrimaryButton("Войти и загрузить контакты") {
                val login = accountInput.text.toString().trim()
                val secret = passwordInput.text.toString()
                if (login.isBlank() || secret.isBlank()) {
                    statusLabel.text = "Введите логин и пароль"
                    statusLabel.setTextColor(Color.rgb(0xE3, 0x4B, 0x2F))
                    return@makePrimaryButton
                }
                syncAddressBook(login, secret, statusLabel)
            }, LinearLayout.LayoutParams(MATCH_PARENT, dp(50)))
            col.addView(vSpace(dp(12)))
        } else {
            val row = LinearLayout(this).apply {
                orientation = LinearLayout.HORIZONTAL
                gravity = Gravity.CENTER
            }
            row.addView(makeSecondaryButton("Обновить") {
                syncAddressBook(account, null, statusLabel)
            }, LinearLayout.LayoutParams(0, dp(44), 1f).also { it.setMargins(0, 0, dp(6), 0) })
            row.addView(makeSecondaryButton("Выйти") {
                prefs.edit()
                    .remove(PREF_AB_TOKEN)
                    .remove(PREF_AB_GUID)
                    .remove(PREF_AB_CONTACTS)
                    .apply()
                showContactsScreen()
            }, LinearLayout.LayoutParams(0, dp(44), 1f).also { it.setMargins(dp(6), 0, 0, 0) })
            col.addView(row)
            col.addView(vSpace(dp(12)))
        }

        col.addView(statusLabel, matchWrap())
        col.addView(vSpace(dp(18)))

        val contacts = loadContacts()
        if (contacts.isEmpty()) {
            col.addView(TextView(this).apply {
                text = if (signedIn) "Контактов пока нет. Нажмите «Обновить»." else "После входа здесь появятся устройства из адресной книги."
                setTextColor(textSoft)
                textSize = 14f
                gravity = Gravity.CENTER
            }, matchWrap())
        } else {
            contacts.forEach { contact ->
                col.addView(contactCard(contact))
                col.addView(vSpace(dp(10)))
            }
        }
    }

    private fun syncAddressBook(account: String, passwordOrNull: String?, statusLabel: TextView) {
        statusLabel.text = "Синхронизация..."
        statusLabel.setTextColor(textSoft)

        thread {
            try {
                val api = AddressBookApi(apiUrl())
                val token = passwordOrNull?.let {
                    api.login(account, it, localDeviceId(), deviceUuid())
                } ?: prefs.getString(PREF_AB_TOKEN, "").orEmpty()

                if (token.isBlank()) error("Нет сохранённого токена. Войдите заново.")
                val guid = api.personalAddressBookGuid(token)
                val contacts = api.peers(token, guid)

                prefs.edit()
                    .putString(PREF_AB_ACCOUNT, account)
                    .putString(PREF_AB_TOKEN, token)
                    .putString(PREF_AB_GUID, guid)
                    .putString(PREF_AB_CONTACTS, contactsToJson(contacts).toString())
                    .apply()

                runOnUiThread {
                    statusLabel.text = "Загружено контактов: ${contacts.size}"
                    statusLabel.setTextColor(brandGreen)
                    showContactsScreen()
                }
            } catch (err: Throwable) {
                runOnUiThread {
                    statusLabel.text = err.message ?: "Ошибка адресной книги"
                    statusLabel.setTextColor(Color.rgb(0xE3, 0x4B, 0x2F))
                }
            }
        }
    }

    private fun contactCard(contact: AddressBookContact): LinearLayout =
        LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            background = roundedBg(cardBg, 18, lineSoft)
            setPadding(dp(14), dp(12), dp(14), dp(12))

            addView(TextView(this@MainActivity).apply {
                text = contact.name.ifBlank { "Без имени" }
                setTextColor(textMain)
                textSize = 17f
                typeface = Typeface.DEFAULT_BOLD
            }, matchWrap())

            addView(TextView(this@MainActivity).apply {
                text = "ID: ${contact.remoteId}" +
                    contact.os.takeIf { it.isNotBlank() }?.let { "  •  $it" }.orEmpty()
                setTextColor(textSoft)
                textSize = 13f
            }, matchWrap())

            if (contact.note.isNotBlank()) {
                addView(TextView(this@MainActivity).apply {
                    text = contact.note
                    setTextColor(textSoft)
                    textSize = 12f
                }, matchWrap())
            }

            addView(vSpace(dp(10)))
            addView(makePrimaryButton("Подключиться") {
                prefs.edit().putString(PREF_LAST_ID, contact.remoteId).apply()
                val status = TextView(this@MainActivity)
                connect(contact.remoteId, "", status)
            }, LinearLayout.LayoutParams(MATCH_PARENT, dp(44)))
        }

    // ── Настройки ────────────────────────────────────────────────────────────
    private fun showSettingsScreen() {
        val col = showAppScreen(
            active = "settings",
            title = "Настройки",
            subtitle = "Встроенный профиль скрыт, свои серверы можно указать вручную",
        )

        val customApi = customSetting(PREF_API_URL)
        val customIdServer = customSetting(PREF_ID_SERVER)
        val customRelay = customSetting(PREF_RELAY_SERVER)
        val customKey = customSetting(PREF_PUBLIC_KEY)
        val usingBuiltInProfile = listOf(customApi, customIdServer, customRelay, customKey)
            .all { it.isBlank() }

        col.addView(TextView(this).apply {
            text = if (usingBuiltInProfile) {
                "Активен встроенный профиль EvertyDesk: ********"
            } else {
                "Активен пользовательский профиль"
            }
            setTextColor(if (usingBuiltInProfile) textSoft else brandGreen)
            textSize = 14f
            gravity = Gravity.CENTER
        }, matchWrap())
        col.addView(vSpace(dp(16)))

        val apiInput = makeInput("********", false).apply { setText(customApi) }
        val idServerInput = makeInput("********", false).apply { setText(customIdServer) }
        val relayInput = makeInput("********", false).apply { setText(customRelay) }
        val keyInput = makeInput("********", false).apply {
            setText(customKey)
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS
        }

        col.addView(label("API адрес"))
        col.addView(apiInput, matchWrap())
        col.addView(vSpace(dp(10)))
        col.addView(label("ID server"))
        col.addView(idServerInput, matchWrap())
        col.addView(vSpace(dp(10)))
        col.addView(label("Relay server"))
        col.addView(relayInput, matchWrap())
        col.addView(vSpace(dp(10)))
        col.addView(label("Public key"))
        col.addView(keyInput, matchWrap())
        col.addView(vSpace(dp(18)))

        val statusLabel = TextView(this).apply {
            text = " "
            setTextColor(textSoft)
            textSize = 13f
            gravity = Gravity.CENTER
        }

        col.addView(makePrimaryButton("Сохранить") {
            val api = apiInput.text.toString().trim()
            val idServer = idServerInput.text.toString().trim()
            val relay = relayInput.text.toString().trim()
            val key = keyInput.text.toString().trim()
            val allBlank = listOf(api, idServer, relay, key).all { it.isBlank() }
            val allFilled = listOf(api, idServer, relay, key).all { it.isNotBlank() }

            if (!allBlank && !allFilled) {
                statusLabel.text = "Заполните все поля своего профиля или очистите все"
                statusLabel.setTextColor(Color.rgb(0xE3, 0x4B, 0x2F))
                return@makePrimaryButton
            }

            if (allBlank) {
                clearCustomServerSettings()
                statusLabel.text = "Используется скрытый профиль EvertyDesk"
            } else {
                prefs.edit()
                    .putString(PREF_API_URL, api)
                    .putString(PREF_ID_SERVER, idServer)
                    .putString(PREF_RELAY_SERVER, relay)
                    .putString(PREF_PUBLIC_KEY, key)
                    .apply()
                statusLabel.text = "Пользовательский профиль сохранён"
            }
            statusLabel.setTextColor(brandGreen)
        }, LinearLayout.LayoutParams(MATCH_PARENT, dp(50)))

        col.addView(vSpace(dp(10)))
        col.addView(makeSecondaryButton("Скрыть и использовать EvertyDesk") {
            clearCustomServerSettings()
            showSettingsScreen()
        }, LinearLayout.LayoutParams(MATCH_PARENT, dp(46)))
        col.addView(vSpace(dp(12)))
        col.addView(statusLabel, matchWrap())
    }

    // ── О нас / контакты ─────────────────────────────────────────────────────
    private fun showAboutScreen() {
        val col = showAppScreen(
            active = "about",
            title = "О нас",
            subtitle = "EvertyDesk Lite для удалённой поддержки и стриминга",
        )

        col.addView(TextView(this).apply {
            text = "EvertyDesk Lite — RustDesk-совместимый клиент с Android-подключением, адресной книгой, терминальным сценарием и упором на быстрый видеопуть."
            setTextColor(textMain)
            textSize = 15f
            setLineSpacing(0f, 1.15f)
        }, matchWrap())

        col.addView(vSpace(dp(18)))
        col.addView(label("Сайт и личный кабинет"))
        col.addView(TextView(this).apply {
            text = defaultApiUrl
            setTextColor(brandGreen)
            textSize = 18f
            typeface = Typeface.DEFAULT_BOLD
        }, matchWrap())
        col.addView(vSpace(dp(14)))
        col.addView(makePrimaryButton("Открыть desk.everty.ru") {
            openWebsite(defaultApiUrl)
        }, LinearLayout.LayoutParams(MATCH_PARENT, dp(50)))
    }

    // ── Подключение ───────────────────────────────────────────────────────────
    private fun connect(id: String, password: String, statusLabel: TextView) {
        if (!client.start(id, password, apiUrl(), idServer(), relayServer(), publicKey())) {
            statusLabel.text = "Не удалось запустить сессию"
            statusLabel.setTextColor(Color.rgb(0xE3, 0x4B, 0x2F))
            return
        }
        rememberRecentSession(id)
        showRemoteScreen()
    }

    // ── Удалённый экран ───────────────────────────────────────────────────────
    private fun showRemoteScreen() {
        root.removeAllViews()
        kbVisible = false

        val rv = RemoteView(this, client).apply {
            layoutParams = FrameLayout.LayoutParams(MATCH_PARENT, MATCH_PARENT)
        }
        remoteView = rv
        root.addView(rv)

        // Скрытый EditText-прокси для клавиатуры
        val proxy = buildKeyProxy()
        keyProxy = proxy
        root.addView(proxy, FrameLayout.LayoutParams(1, 1).also { it.gravity = Gravity.TOP or Gravity.START })

        // Панель спецклавиш (скрыта до нажатия ⌨)
        val specRow = buildSpecialKeysRow()
        kbPanel = specRow

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
            rcBtn.background = roundedBg(
                if (rightClickPending) Color.rgb(0xCC, 0x55, 0x11)
                else Color.rgb(0x22, 0x26, 0x2A),
                14
            )
        }

        val kbBtn = makeToolBtn("⌨  Клав.", Color.rgb(0x22, 0x44, 0x55))
        kbBtn.setOnClickListener { toggleKeyboard() }

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

        // Вертикальный контейнер: [строка спецклавиш][тулбар] — прижат к низу
        val bottomBar = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
        }
        bottomBar.addView(specRow, LinearLayout.LayoutParams(MATCH_PARENT, WRAP_CONTENT))
        bottomBar.addView(toolbar, LinearLayout.LayoutParams(MATCH_PARENT, WRAP_CONTENT))

        root.addView(bottomBar, FrameLayout.LayoutParams(MATCH_PARENT, WRAP_CONTENT,
            Gravity.BOTTOM))

        // callback: если ПКМ-режим активен — следующий тап = правый клик
        rv.setRightClickCallback { x, y ->
            if (rightClickPending) {
                client.rightClick(x, y)
                rightClickPending = false
                rcBtn.background = roundedBg(Color.rgb(0x22, 0x26, 0x2A), 14)
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
    private fun showAppScreen(active: String, title: String, subtitle: String): LinearLayout {
        root.removeAllViews()
        root.setBackgroundColor(brandBg)

        val outer = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setBackgroundColor(brandBg)
            setPadding(dp(18), dp(20), dp(18), dp(12))
        }

        if (active == "connect") {
            outer.addView(ImageView(this).apply {
                setImageResource(R.drawable.edesk_lite_logo)
                adjustViewBounds = true
                scaleType = ImageView.ScaleType.FIT_CENTER
            }, LinearLayout.LayoutParams(dp(82), dp(82)).also {
                it.gravity = Gravity.CENTER_HORIZONTAL
                it.setMargins(0, 0, 0, dp(8))
            })
        }

        outer.addView(TextView(this).apply {
            text = title
            setTextColor(textMain)
            textSize = 28f
            typeface = Typeface.DEFAULT_BOLD
            gravity = Gravity.CENTER
        }, matchWrap())

        outer.addView(TextView(this).apply {
            text = subtitle
            setTextColor(textSoft)
            textSize = 13f
            gravity = Gravity.CENTER
        }, matchWrap())

        outer.addView(vSpace(dp(14)))
        outer.addView(navRow(active), matchWrap())
        outer.addView(vSpace(dp(18)))

        val content = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            gravity = Gravity.CENTER_HORIZONTAL
            setPadding(dp(8), 0, dp(8), dp(24))
        }

        outer.addView(ScrollView(this).apply {
            addView(content, matchWrap())
            isFillViewport = false
        }, LinearLayout.LayoutParams(MATCH_PARENT, 0, 1f))

        root.addView(outer, FrameLayout.LayoutParams(MATCH_PARENT, MATCH_PARENT))
        return content
    }

    private fun navRow(active: String): HorizontalScrollView {
        val row = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER
        }
        row.addView(navButton("Подключение", active == "connect") { showConnectScreen() })
        row.addView(navButton("Контакты", active == "contacts") { showContactsScreen() })
        row.addView(navButton("Настройки", active == "settings") { showSettingsScreen() })
        row.addView(navButton("О нас", active == "about") { showAboutScreen() })
        return HorizontalScrollView(this).apply {
            addView(row)
            isHorizontalScrollBarEnabled = false
        }
    }

    private fun navButton(text: String, selected: Boolean, onClick: () -> Unit): Button =
        Button(this).apply {
            this.text = text
            setTextColor(if (selected) Color.WHITE else textMain)
            background = if (selected) {
                roundedBg(brandGreen, 999)
            } else {
                roundedBg(cardBg, 999, lineSoft)
            }
            textSize = 12f
            setPadding(dp(10), 0, dp(10), 0)
            isAllCaps = false
            setOnClickListener { onClick() }
            layoutParams = LinearLayout.LayoutParams(WRAP_CONTENT, dp(40)).also {
                it.setMargins(dp(3), 0, dp(3), 0)
            }
        }

    private fun makeInput(hintText: String, password: Boolean): EditText =
        EditText(this).apply {
            hint = hintText
            inputType = if (password) {
                InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_PASSWORD
            } else {
                InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS
            }
            setTextColor(textMain)
            setHintTextColor(Color.rgb(0x8A, 0x94, 0x90))
            background = roundedBg(cardBg, 18, lineSoft)
            setPadding(dp(16), dp(12), dp(16), dp(12))
            textSize = 16f
        }

    private fun makePrimaryButton(text: String, onClick: () -> Unit): Button =
        Button(this).apply {
            this.text = text
            setTextColor(Color.WHITE)
            background = roundedBg(brandGreen, 18)
            textSize = 15f
            isAllCaps = false
            setOnClickListener { onClick() }
        }

    private fun makeSecondaryButton(text: String, onClick: () -> Unit): Button =
        Button(this).apply {
            this.text = text
            setTextColor(textMain)
            background = roundedBg(cardBg, 18, lineSoft)
            textSize = 14f
            isAllCaps = false
            setOnClickListener { onClick() }
        }

    private fun label(text: String): TextView =
        TextView(this).apply {
            this.text = text
            setTextColor(textSoft)
            textSize = 12f
        }

    private fun sectionHeader(text: String): TextView =
        TextView(this).apply {
            this.text = text
            setTextColor(textMain)
            textSize = 15f
            typeface = Typeface.DEFAULT_BOLD
            gravity = Gravity.START
        }

    private fun roundedBg(fill: Int, radiusDp: Int, stroke: Int? = null, strokeDp: Int = 1): GradientDrawable =
        GradientDrawable().apply {
            shape = GradientDrawable.RECTANGLE
            setColor(fill)
            cornerRadius = dp(radiusDp).toFloat()
            stroke?.let { setStroke(dp(strokeDp), it) }
        }

    private fun loadRecentSessions(): List<String> {
        val raw = prefs.getString(PREF_RECENT_SESSIONS, "").orEmpty()
        val ids = mutableListOf<String>()
        if (raw.isNotBlank()) {
            try {
                val array = JSONArray(raw)
                for (i in 0 until array.length()) {
                    val id = array.optString(i).filter { it.isDigit() }
                    if (id.isNotBlank() && id !in ids) ids += id
                }
            } catch (_: Throwable) {
                val id = raw.filter { it.isDigit() }
                if (id.isNotBlank()) ids += id
            }
        }
        val last = prefs.getString(PREF_LAST_ID, "").orEmpty().filter { it.isDigit() }
        if (last.isNotBlank() && last !in ids) ids += last
        return ids.take(5)
    }

    private fun rememberRecentSession(remoteId: String) {
        val id = remoteId.filter { it.isDigit() }
        if (id.isBlank()) return
        val recent = mutableListOf(id)
        loadRecentSessions()
            .filter { it != id }
            .take(4)
            .forEach { recent += it }
        val array = JSONArray().also { out ->
            recent.forEach { out.put(it) }
        }
        prefs.edit()
            .putString(PREF_LAST_ID, id)
            .putString(PREF_RECENT_SESSIONS, array.toString())
            .apply()
    }

    private fun customSetting(key: String): String =
        prefs.getString(key, "").orEmpty()

    private fun apiUrl(): String =
        customSetting(PREF_API_URL).ifBlank { defaultApiUrl }

    private fun idServer(): String =
        customSetting(PREF_ID_SERVER).ifBlank { defaultIdServer }

    private fun relayServer(): String =
        customSetting(PREF_RELAY_SERVER).ifBlank { defaultRelayServer }

    private fun publicKey(): String =
        customSetting(PREF_PUBLIC_KEY).ifBlank { defaultPublicKey }

    private fun clearCustomServerSettings() {
        prefs.edit()
            .remove(PREF_API_URL)
            .remove(PREF_ID_SERVER)
            .remove(PREF_RELAY_SERVER)
            .remove(PREF_PUBLIC_KEY)
            .apply()
    }

    private fun deviceUuid(): String {
        val existing = prefs.getString(PREF_DEVICE_UUID, "").orEmpty()
        if (existing.isNotBlank()) return existing
        val generated = UUID.randomUUID().toString()
        prefs.edit().putString(PREF_DEVICE_UUID, generated).apply()
        return generated
    }

    private fun localDeviceId(): String {
        val existing = prefs.getString(PREF_LOCAL_ID, "").orEmpty()
        if (existing.isNotBlank()) return existing
        val seed = deviceUuid().hashCode().toLong() and 0x7FFFFFFFL
        val generated = ((seed % 900_000_000L) + 100_000_000L).toString()
        prefs.edit().putString(PREF_LOCAL_ID, generated).apply()
        return generated
    }

    private fun contactsToJson(contacts: List<AddressBookContact>): JSONArray =
        JSONArray().also { array ->
            contacts.forEach { contact ->
                array.put(
                    JSONObject()
                        .put("name", contact.name)
                        .put("remote_id", contact.remoteId)
                        .put("note", contact.note)
                        .put("os", contact.os)
                        .put("online", contact.online)
                )
            }
        }

    private fun loadContacts(): List<AddressBookContact> {
        val raw = prefs.getString(PREF_AB_CONTACTS, "").orEmpty()
        if (raw.isBlank()) return emptyList()
        return try {
            val array = JSONArray(raw)
            val contacts = mutableListOf<AddressBookContact>()
            for (i in 0 until array.length()) {
                val item = array.optJSONObject(i) ?: continue
                val remoteId = item.optString("remote_id").filter { it.isDigit() }
                if (remoteId.isBlank()) continue
                contacts += AddressBookContact(
                    name = item.optString("name"),
                    remoteId = remoteId,
                    note = item.optString("note"),
                    os = item.optString("os"),
                    online = item.optBoolean("online", false),
                    )
            }
            contacts
        } catch (_: Throwable) {
            emptyList()
        }
    }

    private fun openWebsite(url: String) {
        try {
            startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url)))
        } catch (_: Throwable) {
            Toast.makeText(this, url, Toast.LENGTH_LONG).show()
        }
    }

    private fun dp(v: Int) = (v * resources.displayMetrics.density).toInt()

    /** Вертикальный пробел для LinearLayout с ориентацией VERTICAL */
    private fun vSpace(h: Int) = View(this).apply {
        layoutParams = LinearLayout.LayoutParams(MATCH_PARENT, h)
    }

    private fun matchWrap() = LinearLayout.LayoutParams(MATCH_PARENT, WRAP_CONTENT)

    private fun makeToolBtn(text: String, bg: Int) = Button(this).apply {
        this.text = text
        setTextColor(Color.WHITE)
        background = roundedBg(bg, 14)
        textSize = 11f
        isAllCaps = false
        setPadding(dp(4), 0, dp(4), 0)
    }

    private fun toggleKeyboard() {
        val panel = kbPanel ?: return
        kbVisible = !kbVisible
        panel.visibility = if (kbVisible) View.VISIBLE else View.GONE
        val imm = getSystemService(INPUT_METHOD_SERVICE) as InputMethodManager
        if (kbVisible) {
            keyProxy?.requestFocus()
            imm.showSoftInput(keyProxy, InputMethodManager.SHOW_IMPLICIT)
        } else {
            imm.hideSoftInputFromWindow(keyProxy?.windowToken, 0)
        }
    }

    /** Создаём скрытый EditText-прокси для перехвата ввода клавиатуры. */
    private fun buildKeyProxy(): EditText {
        val SENTINEL = "  " // 2 пробела-заглушки, курсор в середине
        var ignoring = false
        return EditText(this).apply {
            // Прозрачный, размер 0 — только для получения событий IME
            setBackgroundColor(Color.TRANSPARENT)
            setTextColor(Color.TRANSPARENT)
            layoutParams = LinearLayout.LayoutParams(1, 1)
            inputType = InputType.TYPE_CLASS_TEXT or
                        InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS or
                        InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD
            isSingleLine = true
            setText(SENTINEL)
            setSelection(1)

            addTextChangedListener(object : TextWatcher {
                override fun beforeTextChanged(s: CharSequence?, st: Int, c: Int, a: Int) {}
                override fun onTextChanged(s: CharSequence?, st: Int, b: Int, c: Int) {}
                override fun afterTextChanged(s: Editable) {
                    if (ignoring) return
                    val text = s.toString()
                    when {
                        text.length > SENTINEL.length -> {
                            // Напечатан текст — всё что между первым и последним символом
                            val added = text.drop(1).dropLast(1)
                            if (added.isNotEmpty()) client.keyText(added)
                        }
                        text.length < SENTINEL.length -> {
                            // Удалён символ — Backspace
                            client.keyControl(2)
                        }
                    }
                    ignoring = true
                    s.replace(0, s.length, SENTINEL)
                    setSelection(1)
                    ignoring = false
                }
            })

            // Перехватываем спецклавиши (стрелки, Enter от hardware keyboard)
            setOnKeyListener { _, keyCode, event ->
                if (event.action == KeyEvent.ACTION_DOWN) {
                    val ck = androidKeyToControlKey(keyCode)
                    if (ck != 0) { client.keyControl(ck); return@setOnKeyListener true }
                }
                false
            }
        }
    }

    private fun androidKeyToControlKey(keyCode: Int): Int = when (keyCode) {
        KeyEvent.KEYCODE_ENTER, KeyEvent.KEYCODE_NUMPAD_ENTER -> 27   // Return
        KeyEvent.KEYCODE_ESCAPE                               -> 8    // Escape
        KeyEvent.KEYCODE_TAB                                  -> 31   // Tab
        KeyEvent.KEYCODE_DPAD_UP                              -> 32   // UpArrow
        KeyEvent.KEYCODE_DPAD_DOWN                            -> 6    // DownArrow
        KeyEvent.KEYCODE_DPAD_LEFT                            -> 22   // LeftArrow
        KeyEvent.KEYCODE_DPAD_RIGHT                           -> 28   // RightArrow
        KeyEvent.KEYCODE_DEL                                  -> 2    // Backspace
        KeyEvent.KEYCODE_FORWARD_DEL                          -> 5    // Delete
        KeyEvent.KEYCODE_HOME                                 -> 21   // Home
        KeyEvent.KEYCODE_MOVE_END                             -> 7    // End
        KeyEvent.KEYCODE_PAGE_UP                              -> 26   // PageUp
        KeyEvent.KEYCODE_PAGE_DOWN                            -> 25   // PageDown
        else                                                  -> 0
    }

    /** Строка со спецклавишами поверх тулбара. */
    private fun buildSpecialKeysRow(): HorizontalScrollView {
        data class SK(val label: String, val ck: Int)
        val keys = listOf(
            SK("Esc", 8), SK("Tab", 31), SK("↑", 32), SK("↓", 6),
            SK("←", 22), SK("→", 28), SK("Home", 21), SK("End", 7),
            SK("PgUp", 26), SK("PgDn", 25), SK("Del", 5),
            SK("Ctrl+C", -1), SK("Ctrl+V", -2), SK("Ctrl+A", -3), SK("Ctrl+Z", -4)
        )
        val row = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            background = roundedBg(Color.argb(0xEE, 0x0D, 0x12, 0x10), 14)
            setPadding(dp(4), dp(4), dp(4), dp(4))
        }
        for (sk in keys) {
            val btn = Button(this).apply {
                text = sk.label
                setTextColor(Color.WHITE)
                textSize = 10f
                background = roundedBg(Color.rgb(0x22, 0x26, 0x2A), 12)
                isAllCaps = false
                setPadding(dp(6), dp(2), dp(6), dp(2))
                setOnClickListener {
                    when (sk.ck) {
                        -1 -> client.keyCtrl("c")
                        -2 -> client.keyCtrl("v")
                        -3 -> client.keyCtrl("a")
                        -4 -> client.keyCtrl("z")
                        else -> client.keyControl(sk.ck)
                    }
                    keyProxy?.requestFocus()
                }
            }
            row.addView(btn, LinearLayout.LayoutParams(WRAP_CONTENT, dp(34)).also {
                it.setMargins(dp(2), 0, dp(2), 0)
            })
        }
        return HorizontalScrollView(this).apply {
            addView(row)
            isHorizontalScrollBarEnabled = false
            visibility = View.GONE
        }
    }
}
