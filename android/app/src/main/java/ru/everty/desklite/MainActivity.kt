package ru.everty.desklite

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.content.pm.ActivityInfo
import android.content.res.Configuration
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
import android.graphics.SurfaceTexture
import android.view.Surface
import android.view.TextureView
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
import androidx.security.crypto.EncryptedSharedPreferences
import androidx.security.crypto.MasterKeys
import org.json.JSONArray
import org.json.JSONObject
import java.util.UUID
import kotlin.concurrent.thread

class MainActivity : Activity() {
    private val client = NativeClient()
    private lateinit var root: FrameLayout
    private var remoteView: RemoteView? = null
    private var touchpadView: TouchpadView? = null
    private var audioPlayer: EvrtAudioPlayer? = null
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
    private var rotateBtn: Button? = null

    // Скрытый EditText-прокси для клавиатурного ввода
    private var keyProxy: EditText? = null
    private var kbPanel: View? = null
    private var kbVisible = false

    private val prefs by lazy { getSharedPreferences("everty_prefs", Context.MODE_PRIVATE) }

    // Пароли хранятся в зашифрованных SharedPreferences (Android Keystore, AES-256-GCM).
    // На рутованных устройствах plaintext SharedPreferences читаются любым приложением;
    // EncryptedSharedPreferences защищены аппаратным ключом.
    private val securePrefs by lazy {
        try {
            val keyAlias = MasterKeys.getOrCreate(MasterKeys.AES256_GCM_SPEC)
            EncryptedSharedPreferences.create(
                "everty_secure_prefs",
                keyAlias,
                this,
                EncryptedSharedPreferences.PrefKeyEncryptionScheme.AES256_SIV,
                EncryptedSharedPreferences.PrefValueEncryptionScheme.AES256_GCM,
            )
        } catch (_: Exception) {
            // Keystore недоступен (эмулятор без Google Play?) — fallback на обычные prefs.
            // В проде на реальном устройстве этой ветки не бывает.
            getSharedPreferences("everty_secure_prefs_fallback", Context.MODE_PRIVATE)
        }
    }
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
    private val PREF_AB_LOCAL_CONTACTS = "address_book_local_contacts"
    private val PREF_SAVED_PASSWORDS = "saved_passwords"  // JSON {id: password}
    private val PREF_NATURAL_SCROLL = "natural_scroll"    // true = Mac natural (default)
    private val PREF_CODEC = "codec"                       // EVRTCK | H264 | H265 | AV1

    // ID текущего удалённого хоста — используется для уведомления хоста через агент
    private var currentRemoteId = ""

    // Кодек активной сессии — нужен для выбора между Surface и bitmap режимами
    private var activeCodec = "EVRTCK"

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
            prefs.getString(PREF_LAST_ID, "")?.takeIf { it.isNotEmpty() }?.let {
                setText(it)
                setSelection(it.length)
            }
        }
        col.addView(idInput, matchWrap())

        col.addView(vSpace(dp(12)))

        val pwInput = makeInput("Пароль (необязательно)", true)
        col.addView(pwInput, matchWrap())

        // Строка-подсказка под полем пароля: показывает что пароль сохранён + кнопка очистки
        val pwSavedHint = TextView(this).apply {
            textSize = 12f
            setTextColor(textSoft)
            visibility = View.GONE
        }
        col.addView(pwSavedHint, matchWrap())

        fun refreshPwHint() {
            val id = idInput.text.toString().filter { it.isDigit() }
            val saved = if (id.isNotEmpty()) savedPassword(id) else ""
            if (saved.isNotEmpty()) {
                // Не заполняем поле автоматически — пользователь сам решает.
                // Автозаполнение при смене хоста приводило к отправке старого пароля
                // хосту в режиме "Принять", что вызывало "Wrong Password" даже после
                // того как хост нажимал "Принять".
                pwSavedHint.text = "🔑 Пароль сохранён  ·  Нажмите чтобы подставить"
                pwSavedHint.visibility = View.VISIBLE
                pwSavedHint.setOnClickListener {
                    if (pwInput.text.isNullOrEmpty()) {
                        pwInput.setText(saved)
                        pwSavedHint.text = "🔑 Пароль подставлен  ·  Нажмите чтобы очистить"
                    } else {
                        val rid = idInput.text.toString().filter { it.isDigit() }
                        savePassword(rid, "")
                        pwInput.setText("")
                        pwSavedHint.visibility = View.GONE
                    }
                }
            } else {
                pwSavedHint.visibility = View.GONE
            }
        }
        // Заполняем сразу при открытии и при смене ID
        refreshPwHint()
        idInput.addTextChangedListener(object : TextWatcher {
            override fun afterTextChanged(s: Editable?) = refreshPwHint()
            override fun beforeTextChanged(s: CharSequence?, start: Int, count: Int, after: Int) {}
            override fun onTextChanged(s: CharSequence?, start: Int, before: Int, count: Int) {}
        })

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
        col.addView(makeSecondaryButton("Тачпад без картинки") {
            val id = idInput.text.toString().filter { it.isDigit() }
            if (id.isEmpty()) {
                statusLabel.text = "Введите ID партнёра"
                statusLabel.setTextColor(Color.rgb(0xE3, 0x4B, 0x2F))
                return@makeSecondaryButton
            }
            prefs.edit().putString(PREF_LAST_ID, id).apply()
            statusLabel.text = "Подключение тачпада..."
            statusLabel.setTextColor(textSoft)
            connect(id, pwInput.text.toString(), statusLabel, touchpadOnly = true)
        }, LinearLayout.LayoutParams(MATCH_PARENT, dp(48)))
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

            addView(LinearLayout(this@MainActivity).apply {
                orientation = LinearLayout.HORIZONTAL
                addView(makeSecondaryButton("Экран") {
                    idInput.setText(remoteId)
                    idInput.setSelection(idInput.text.length)
                    connect(remoteId, pwInput.text.toString(), statusLabel)
                }, LinearLayout.LayoutParams(dp(92), dp(42)).also {
                    it.setMargins(0, 0, dp(6), 0)
                })
                addView(makeSecondaryButton("Тач") {
                    idInput.setText(remoteId)
                    idInput.setSelection(idInput.text.length)
                    connect(remoteId, pwInput.text.toString(), statusLabel, touchpadOnly = true)
                }, LinearLayout.LayoutParams(dp(82), dp(42)))
            }, LinearLayout.LayoutParams(WRAP_CONTENT, WRAP_CONTENT))
        }

    // ── Адресная книга ───────────────────────────────────────────────────────
    private fun showContactsScreen() {
        val col = showAppScreen(
            active = "contacts",
            title = "Адресная книга",
            subtitle = "Устройства для быстрого подключения",
        )

        // ── Кнопка добавления ────────────────────────────────────────────────
        col.addView(makePrimaryButton("+ Добавить устройство") {
            showAddDeviceDialog { showContactsScreen() }
        }, LinearLayout.LayoutParams(MATCH_PARENT, dp(48)))
        col.addView(vSpace(dp(18)))

        // ── Синхронизация с сервером ─────────────────────────────────────────
        val account = prefs.getString(PREF_AB_ACCOUNT, "").orEmpty()
        val token = prefs.getString(PREF_AB_TOKEN, "").orEmpty()
        val guid = prefs.getString(PREF_AB_GUID, "").orEmpty()
        val signedIn = token.isNotBlank() && guid.isNotBlank()

        val statusLabel = TextView(this).apply {
            text = if (signedIn) "Синхронизировано: $account" else "Войдите для загрузки облачных контактов"
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

            col.addView(sectionHeader("Синхронизация с EvertyDesk"))
            col.addView(vSpace(dp(8)))
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

        // ── Локальные устройства ─────────────────────────────────────────────
        val localContacts = loadLocalContacts()
        if (localContacts.isNotEmpty()) {
            col.addView(sectionHeader("Мои устройства"))
            col.addView(vSpace(dp(8)))
            localContacts.forEach { contact ->
                col.addView(contactCard(contact, onDelete = {
                    val updated = loadLocalContacts().filter { it.remoteId != contact.remoteId }
                    saveLocalContacts(updated)
                    showContactsScreen()
                }))
                col.addView(vSpace(dp(10)))
            }
            col.addView(vSpace(dp(8)))
        }

        // ── Серверные контакты ───────────────────────────────────────────────
        val serverContacts = loadContacts()
        if (serverContacts.isNotEmpty()) {
            col.addView(sectionHeader("Адресная книга"))
            col.addView(vSpace(dp(8)))
            serverContacts.forEach { contact ->
                col.addView(contactCard(contact))
                col.addView(vSpace(dp(10)))
            }
        }

        if (localContacts.isEmpty() && serverContacts.isEmpty()) {
            col.addView(TextView(this).apply {
                text = "Нажмите «+ Добавить устройство» чтобы сохранить ID для быстрого подключения."
                setTextColor(textSoft)
                textSize = 14f
                gravity = Gravity.CENTER
                setLineSpacing(0f, 1.2f)
            }, matchWrap())
        }
    }

    /** Диалог добавления устройства — оверлей поверх root. */
    private fun showAddDeviceDialog(onSaved: () -> Unit) {
        val overlay = FrameLayout(this).apply {
            setBackgroundColor(Color.argb(0xBB, 0, 0, 0))
            isClickable = true
            isFocusable = true
        }

        val card = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            background = roundedBg(cardBg, 22)
            setPadding(dp(20), dp(20), dp(20), dp(20))
        }

        val nameInput = makeInput("Имя устройства (необязательно)", false)
        val idInput = makeInput("ID устройства *", false).apply {
            inputType = InputType.TYPE_CLASS_NUMBER
        }
        val noteInput = makeInput("Заметка (необязательно)", false)

        val errorLabel = TextView(this).apply {
            text = " "
            setTextColor(Color.rgb(0xE3, 0x4B, 0x2F))
            textSize = 12f
            gravity = Gravity.CENTER
        }

        card.addView(TextView(this).apply {
            text = "Добавить устройство"
            setTextColor(textMain)
            textSize = 18f
            typeface = Typeface.DEFAULT_BOLD
            gravity = Gravity.CENTER
        }, matchWrap())
        card.addView(vSpace(dp(4)))
        card.addView(TextView(this).apply {
            text = "Введите ID партнёра для быстрого доступа"
            setTextColor(textSoft)
            textSize = 12f
            gravity = Gravity.CENTER
        }, matchWrap())
        card.addView(vSpace(dp(16)))
        card.addView(idInput, matchWrap())
        card.addView(vSpace(dp(10)))
        card.addView(nameInput, matchWrap())
        card.addView(vSpace(dp(10)))
        card.addView(noteInput, matchWrap())
        card.addView(vSpace(dp(4)))
        card.addView(errorLabel, matchWrap())
        card.addView(vSpace(dp(12)))

        fun dismiss() {
            root.removeView(overlay)
        }

        val btnRow = LinearLayout(this).apply { orientation = LinearLayout.HORIZONTAL }
        btnRow.addView(makeSecondaryButton("Отмена") {
            dismiss()
        }, LinearLayout.LayoutParams(0, dp(48), 1f).also { it.setMargins(0, 0, dp(6), 0) })
        btnRow.addView(makePrimaryButton("Сохранить") {
            val id = idInput.text.toString().filter { it.isDigit() }
            if (id.isBlank()) {
                errorLabel.text = "Введите ID устройства"
                return@makePrimaryButton
            }
            val existing = loadLocalContacts()
            if (existing.any { it.remoteId == id }) {
                errorLabel.text = "Устройство с ID $id уже добавлено"
                return@makePrimaryButton
            }
            val name = nameInput.text.toString().trim()
            val note = noteInput.text.toString().trim()
            saveLocalContacts(existing + AddressBookContact(
                name = name, remoteId = id, note = note, os = "", online = false
            ))
            dismiss()
            onSaved()
        }, LinearLayout.LayoutParams(0, dp(48), 1f).also { it.setMargins(dp(6), 0, 0, 0) })
        card.addView(btnRow, matchWrap())

        val lp = FrameLayout.LayoutParams(MATCH_PARENT, WRAP_CONTENT, Gravity.CENTER).apply {
            val m = dp(24)
            setMargins(m, m, m, m)
        }
        overlay.addView(card, lp)
        // Клик вне карточки = закрыть
        overlay.setOnClickListener { dismiss() }
        card.setOnClickListener { /* перехватить, не закрывать */ }

        root.addView(overlay, FrameLayout.LayoutParams(MATCH_PARENT, MATCH_PARENT))
        // Фокус сразу на поле ID
        idInput.requestFocus()
    }

    // ── Сохранённые пароли (зашифрованные) ───────────────────────────────────

    private fun savedPassword(id: String): String =
        // Каждый пароль хранится как отдельная запись "pw_<id>" → нет JSON-парсинга в hot path.
        securePrefs.getString("pw_$id", "").orEmpty().also { pw ->
            // Однократная миграция из старого plaintext хранилища
            if (pw.isEmpty()) {
                val legacy = prefs.getString(PREF_SAVED_PASSWORDS, "{}") ?: "{}"
                val old = try { JSONObject(legacy).optString(id, "") } catch (_: Exception) { "" }
                if (old.isNotEmpty()) {
                    securePrefs.edit().putString("pw_$id", old).apply()
                    // Удалить из старого хранилища
                    val obj = try { JSONObject(legacy) } catch (_: Exception) { JSONObject() }
                    obj.remove(id)
                    prefs.edit().putString(PREF_SAVED_PASSWORDS, obj.toString()).apply()
                    return old
                }
            }
        }

    private fun savePassword(id: String, password: String) {
        if (password.isBlank()) {
            securePrefs.edit().remove("pw_$id").apply()
        } else {
            securePrefs.edit().putString("pw_$id", password).apply()
        }
    }

    // ── Уведомление хоста о подключении ──────────────────────────────────────

    /** Сообщает бэкенду что Android подключился к хосту — хост получит баннер через агент. */
    private fun notifySessionConnected(hostRdId: String) {
        if (hostRdId.isEmpty()) return
        val url = "${apiUrl()}/admin/agent/session-event"
        thread {
            try {
                val conn = java.net.URL(url).openConnection() as java.net.HttpURLConnection
                conn.requestMethod = "POST"
                conn.setRequestProperty("Content-Type", "application/json")
                conn.connectTimeout = 6_000
                conn.readTimeout = 6_000
                conn.doOutput = true
                conn.outputStream.bufferedWriter().use {
                    it.write("{\"host_rustdesk_id\":\"$hostRdId\",\"event\":\"connected\"}")
                }
                conn.disconnect()
            } catch (_: Exception) {}
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

    private fun contactCard(contact: AddressBookContact, onDelete: (() -> Unit)? = null): LinearLayout =
        LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            background = roundedBg(cardBg, 18, lineSoft)
            setPadding(dp(14), dp(12), dp(14), dp(12))

            // Шапка: имя + крестик удаления (если локальный контакт)
            addView(LinearLayout(this@MainActivity).apply {
                orientation = LinearLayout.HORIZONTAL
                gravity = Gravity.CENTER_VERTICAL
                addView(TextView(this@MainActivity).apply {
                    text = contact.name.ifBlank { "ID ${contact.remoteId}" }
                    setTextColor(textMain)
                    textSize = 17f
                    typeface = Typeface.DEFAULT_BOLD
                }, LinearLayout.LayoutParams(0, WRAP_CONTENT, 1f))
                if (onDelete != null) {
                    addView(Button(this@MainActivity).apply {
                        text = "✕"
                        setTextColor(Color.rgb(0xCC, 0x44, 0x33))
                        background = roundedBg(Color.rgb(0xFF, 0xF0, 0xEE), 10)
                        textSize = 13f
                        isAllCaps = false
                        setPadding(dp(8), 0, dp(8), 0)
                        setOnClickListener { onDelete() }
                    }, LinearLayout.LayoutParams(dp(36), dp(36)))
                }
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
            addView(LinearLayout(this@MainActivity).apply {
                orientation = LinearLayout.HORIZONTAL
                addView(makePrimaryButton("Экран") {
                    prefs.edit().putString(PREF_LAST_ID, contact.remoteId).apply()
                    val status = TextView(this@MainActivity)
                    connect(contact.remoteId, savedPassword(contact.remoteId), status)
                }, LinearLayout.LayoutParams(0, dp(44), 1f).also {
                    it.setMargins(0, 0, dp(6), 0)
                })
                addView(makeSecondaryButton("Тачпад") {
                    prefs.edit().putString(PREF_LAST_ID, contact.remoteId).apply()
                    val status = TextView(this@MainActivity)
                    connect(contact.remoteId, savedPassword(contact.remoteId), status, touchpadOnly = true)
                }, LinearLayout.LayoutParams(0, dp(44), 1f).also {
                    it.setMargins(dp(6), 0, 0, 0)
                })
            }, LinearLayout.LayoutParams(MATCH_PARENT, WRAP_CONTENT))
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

        // ── Тачпад ────────────────────────────────────────────────────────────
        col.addView(vSpace(dp(24)))
        col.addView(sectionHeader("Тачпад"))
        col.addView(vSpace(dp(12)))

        val naturalScrollRow = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            background = roundedBg(cardBg, 18, lineSoft)
            setPadding(dp(16), dp(14), dp(16), dp(14))
        }
        val naturalScrollSwitch = android.widget.Switch(this).apply {
            isChecked = prefs.getBoolean(PREF_NATURAL_SCROLL, true)
        }
        val naturalScrollLabel = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
        }
        naturalScrollLabel.addView(TextView(this).apply {
            text = "Естественная прокрутка"
            setTextColor(textMain)
            textSize = 15f
        })
        naturalScrollLabel.addView(TextView(this).apply {
            text = "Как на Mac: свайп вниз → страница идёт вниз"
            setTextColor(textSoft)
            textSize = 12f
        })
        naturalScrollRow.addView(naturalScrollLabel,
            LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f))
        naturalScrollRow.addView(naturalScrollSwitch)
        naturalScrollSwitch.setOnCheckedChangeListener { _, isChecked ->
            prefs.edit().putBoolean(PREF_NATURAL_SCROLL, isChecked).apply()
            remoteView?.setNaturalScroll(isChecked)
            touchpadView?.setNaturalScroll(isChecked)
        }
        col.addView(naturalScrollRow, matchWrap())
        col.addView(vSpace(dp(8)))
        col.addView(label("Отключите, если скролл ощущается перевёрнутым на вашем хосте"), matchWrap())

        // ── Кодек ────────────────────────────────────────────────────────────
        col.addView(vSpace(dp(24)))
        col.addView(sectionHeader("Кодек видео"))
        col.addView(vSpace(dp(8)))
        col.addView(TextView(this).apply {
            text = "Выбранный кодек используется при следующем подключении. EVRTCK — лучший выбор для LAN и прямых соединений."
            setTextColor(textSoft)
            textSize = 12f
            setLineSpacing(0f, 1.2f)
        }, matchWrap())
        col.addView(vSpace(dp(12)))

        val codecs = listOf("EVRTCK", "H264", "H265", "AV1")
        val codecLabels = mapOf("EVRTCK" to "EVRTCK\nLAN / прямой", "H264" to "H264\nСовместимость", "H265" to "H265\nКачество/фильмы", "AV1" to "AV1\nЭффективность")
        val codecBtnRow = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER
        }
        val codecBtns = mutableMapOf<String, Button>()

        fun updateCodecButtons(selected: String) {
            codecs.forEach { c ->
                codecBtns[c]?.apply {
                    background = if (c == selected) roundedBg(brandGreen, 14) else roundedBg(cardBg, 14, lineSoft)
                    setTextColor(if (c == selected) Color.WHITE else textMain)
                }
            }
        }

        codecs.forEach { c ->
            val btn = Button(this).apply {
                text = codecLabels[c] ?: c
                textSize = 10f
                isAllCaps = false
                setOnClickListener {
                    prefs.edit().putString(PREF_CODEC, c).apply()
                    updateCodecButtons(c)
                }
            }
            codecBtns[c] = btn
            codecBtnRow.addView(btn, LinearLayout.LayoutParams(0, dp(64), 1f).also {
                it.setMargins(dp(2), 0, dp(2), 0)
            })
        }
        updateCodecButtons(selectedCodec())
        col.addView(codecBtnRow, matchWrap())
    }

    // ── Игровой режим ─────────────────────────────────────────────────────────
    private fun showGameScreen() {
        val col = showAppScreen(
            active = "game",
            title = "Игра",
            subtitle = "H265 / EVRT — стриминг с низкой задержкой",
        )

        val infoCard = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            background = roundedBg(Color.rgb(0x0D, 0x20, 0x18), 18, Color.rgb(0x12, 0xC9, 0x72))
            setPadding(dp(16), dp(14), dp(16), dp(14))
        }
        infoCard.addView(TextView(this).apply {
            text = "🎮  Игровой стриминг"
            setTextColor(brandGreen)
            textSize = 15f
            typeface = Typeface.DEFAULT_BOLD
        }, matchWrap())
        infoCard.addView(vSpace(dp(6)))
        infoCard.addView(TextView(this).apply {
            text = "Использует H265 через EVRT UDP — минимальная задержка, высокое качество. H265 эффективнее сжимает и даёт лучшую картинку при том же битрейте."
            setTextColor(Color.rgb(0xCC, 0xFF, 0xE0))
            textSize = 12f
            setLineSpacing(0f, 1.2f)
        }, matchWrap())
        col.addView(infoCard, matchWrap())
        col.addView(vSpace(dp(20)))

        val idInput = makeInput("ID партнёра", false).apply {
            inputType = InputType.TYPE_CLASS_NUMBER
            textSize = 18f
            prefs.getString(PREF_LAST_ID, "")?.takeIf { it.isNotEmpty() }?.let {
                setText(it)
                setSelection(it.length)
            }
        }
        col.addView(idInput, matchWrap())
        col.addView(vSpace(dp(10)))
        val pwInput = makeInput("Пароль (необязательно)", true)
        col.addView(pwInput, matchWrap())
        col.addView(vSpace(dp(20)))

        // Выбор кодека для игрового режима
        col.addView(label("Кодек (рекомендуется H265)"))
        col.addView(vSpace(dp(8)))
        val gameCodecs = listOf("H265", "AV1", "EVRTCK")
        var gameCodec = prefs.getString("game_codec", "H265") ?: "H265"
        val gameCodecRow = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
        }
        val gameCodecBtns = mutableMapOf<String, Button>()

        fun updateGameCodecBtns(sel: String) {
            gameCodecs.forEach { c ->
                gameCodecBtns[c]?.apply {
                    background = if (c == sel) roundedBg(brandGreen, 14) else roundedBg(cardBg, 14, lineSoft)
                    setTextColor(if (c == sel) Color.WHITE else textMain)
                }
            }
        }

        gameCodecs.forEach { c ->
            val btn = Button(this).apply {
                text = c
                textSize = 11f
                isAllCaps = false
                setOnClickListener {
                    gameCodec = c
                    prefs.edit().putString("game_codec", c).apply()
                    updateGameCodecBtns(c)
                }
            }
            gameCodecBtns[c] = btn
            gameCodecRow.addView(btn, LinearLayout.LayoutParams(0, dp(44), 1f).also {
                it.setMargins(dp(2), 0, dp(2), 0)
            })
        }
        updateGameCodecBtns(gameCodec)
        col.addView(gameCodecRow, matchWrap())
        col.addView(vSpace(dp(20)))

        val statusLabel = TextView(this).apply {
            text = " "
            setTextColor(textSoft)
            textSize = 13f
            gravity = Gravity.CENTER
        }

        col.addView(makePrimaryButton("▶  Запустить игровой стрим") {
            val id = idInput.text.toString().filter { it.isDigit() }
            if (id.isEmpty()) {
                statusLabel.text = "Введите ID партнёра"
                statusLabel.setTextColor(Color.rgb(0xE3, 0x4B, 0x2F))
                return@makePrimaryButton
            }
            prefs.edit().putString(PREF_LAST_ID, id).apply()
            statusLabel.text = "Подключение (${gameCodec})..."
            statusLabel.setTextColor(textSoft)
            val pw = pwInput.text.toString()
            val dm = android.util.DisplayMetrics()
            @Suppress("DEPRECATION")
            windowManager.defaultDisplay.getRealMetrics(dm)
            client.setMaxResolution(dm.widthPixels, dm.heightPixels)
            val started = client.start(id, pw, apiUrl(), idServer(), relayServer(), publicKey(), gameCodec)
            if (!started) {
                statusLabel.text = "Не удалось запустить сессию"
                statusLabel.setTextColor(Color.rgb(0xE3, 0x4B, 0x2F))
                return@makePrimaryButton
            }
            rememberRecentSession(id)
            if (pw.isNotEmpty()) savePassword(id, pw)
            currentRemoteId = id
            activeCodec = gameCodec
            showGameRemoteScreen()
        }.apply { textSize = 16f }, LinearLayout.LayoutParams(MATCH_PARENT, dp(56)))

        col.addView(vSpace(dp(12)))
        col.addView(statusLabel, matchWrap())
    }

    // ── Удалённый экран для игрового режима ───────────────────────────────────
    private fun showGameRemoteScreen() {
        root.removeAllViews()
        kbVisible = false
        touchpadView = null

        val useHardware = activeCodec != "EVRTCK"

        // TextureView — unlike SurfaceView, resize doesn't destroy/recreate the surface,
        // so the decoder survives onVideoRectChanged without a 2-second IDR wait.
        var textureView: TextureView? = null
        if (useHardware) {
            val tv = TextureView(this).apply {
                layoutParams = FrameLayout.LayoutParams(MATCH_PARENT, MATCH_PARENT)
                keepScreenOn = true
            }
            textureView = tv
            root.addView(tv)
        }

        val rv = RemoteView(this, client).apply {
            layoutParams = FrameLayout.LayoutParams(MATCH_PARENT, MATCH_PARENT)
            setNaturalScroll(false)
            isGameMode = useHardware
            if (useHardware) {
                isHardwareDecodeMode = true
                setBackgroundColor(Color.TRANSPARENT)
                onVideoRectChanged = { rect ->
                    textureView?.layoutParams = FrameLayout.LayoutParams(rect.width, rect.height).apply {
                        leftMargin = rect.x
                        topMargin = rect.y
                    }
                }
            }
        }
        remoteView = rv
        root.addView(rv)

        if (useHardware) {
            VideoDecoder.onDimensionsAvailable = { w, h ->
                handler.post { rv.setHardwareDimensions(w, h) }
            }
            textureView?.surfaceTextureListener = object : TextureView.SurfaceTextureListener {
                override fun onSurfaceTextureAvailable(st: SurfaceTexture, w: Int, h: Int) {
                    // Re-arm onDimensionsAvailable — releaseAll() might have cleared it
                    VideoDecoder.onDimensionsAvailable = { dw, dh ->
                        handler.post { rv.setHardwareDimensions(dw, dh) }
                    }
                    VideoDecoder.setSurface(Surface(st))
                }
                override fun onSurfaceTextureSizeChanged(st: SurfaceTexture, w: Int, h: Int) {}
                override fun onSurfaceTextureDestroyed(st: SurfaceTexture): Boolean {
                    VideoDecoder.releaseAll()
                    return true
                }
                override fun onSurfaceTextureUpdated(st: SurfaceTexture) {}
            }
        }

        val proxy = buildKeyProxy()
        keyProxy = proxy
        root.addView(proxy, FrameLayout.LayoutParams(1, 1).also { it.gravity = Gravity.TOP or Gravity.START })

        // Статус-оверлей
        val overlay = TextView(this).apply {
            setTextColor(Color.WHITE)
            setBackgroundColor(Color.argb(0xAA, 0, 0, 0))
            textSize = 11f
            setPadding(dp(10), dp(6), dp(10), dp(6))
        }
        root.addView(overlay, FrameLayout.LayoutParams(WRAP_CONTENT, WRAP_CONTENT, Gravity.TOP or Gravity.START))

        // Геймпад-оверлей (скрыт по умолчанию)
        val gamepadOverlay = buildGamepadOverlay()
        var gamepadVisible = false
        root.addView(gamepadOverlay, FrameLayout.LayoutParams(MATCH_PARENT, MATCH_PARENT, Gravity.BOTTOM))
        gamepadOverlay.visibility = View.GONE

        // Тулбар
        val toolbar = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            setBackgroundColor(toolbarBg)
            gravity = Gravity.CENTER_VERTICAL
            setPadding(dp(6), dp(6), dp(6), dp(6))
        }

        val gamepadBtn = makeToolBtn("🎮  Геймпад", Color.rgb(0x22, 0x33, 0x55))
        gamepadBtn.setOnClickListener {
            gamepadVisible = !gamepadVisible
            gamepadOverlay.visibility = if (gamepadVisible) View.VISIBLE else View.GONE
            gamepadBtn.background = roundedBg(
                if (gamepadVisible) brandGreen else Color.rgb(0x22, 0x33, 0x55), 14)
        }

        val kbBtn = makeToolBtn("⌨  Клав.", Color.rgb(0x22, 0x44, 0x55))
        kbBtn.setOnClickListener { toggleKeyboard() }

        val zoomBtn = makeToolBtn("⊞  1:1", Color.rgb(0x22, 0x44, 0x33))
        zoomBtn.setOnClickListener { rv.resetZoom() }

        val rotBtn = makeRotateBtn()

        val muteBtn = makeToolBtn("🔊  Звук", Color.rgb(0x22, 0x44, 0x44))
        muteBtn.setOnClickListener {
            val player = audioPlayer ?: return@setOnClickListener
            player.muted = !player.muted
            if (player.muted) {
                muteBtn.text = "🔇  Звук"
                muteBtn.background = roundedBg(Color.rgb(0x55, 0x33, 0x22), 14)
            } else {
                muteBtn.text = "🔊  Звук"
                muteBtn.background = roundedBg(Color.rgb(0x22, 0x44, 0x44), 14)
            }
        }

        val discBtn = makeToolBtn("✕  Выход", Color.rgb(0x66, 0x22, 0x22))
        discBtn.setOnClickListener { disconnect() }

        listOf(gamepadBtn, kbBtn, zoomBtn, rotBtn, muteBtn, discBtn).forEach { btn ->
            toolbar.addView(btn, LinearLayout.LayoutParams(0, dp(40), 1f).also {
                it.setMargins(dp(3), 0, dp(3), 0)
            })
        }

        val specRow = buildSpecialKeysRow()
        kbPanel = specRow
        val bottomBar = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }
        bottomBar.addView(specRow, LinearLayout.LayoutParams(MATCH_PARENT, WRAP_CONTENT))
        bottomBar.addView(toolbar, LinearLayout.LayoutParams(MATCH_PARENT, WRAP_CONTENT))
        root.addView(bottomBar, FrameLayout.LayoutParams(MATCH_PARENT, WRAP_CONTENT, Gravity.BOTTOM))

        rv.setRightClickCallback { _, _ -> false }
        rv.startRendering()

        // Запускаем аудио плеер для игрового режима
        audioPlayer?.stop()
        audioPlayer = EvrtAudioPlayer(client).also { it.start() }

        val statusTick = object : Runnable {
            var hostNotified = false
            var authWatchTicks = 0
            override fun run() {
                val connected = client.isConnected()
                val status = client.status()
                if (!connected) {
                    authWatchTicks++
                    if (authWatchTicks in 2..30 && looksLikeAuthError(status)) {
                        handleAuthFailure(currentRemoteId)
                        return
                    }
                } else {
                    authWatchTicks = 99
                }
                val statusText = if (connected) "● $status" else status
                overlay.text = if (useHardware && connected) {
                    "$statusText  |  ${PerfStats.summary()}"
                } else {
                    statusText
                }
                if (connected && !hostNotified) {
                    hostNotified = true
                    notifySessionConnected(currentRemoteId)
                }
                handler.postDelayed(this, 500)
            }
        }
        handler.post(statusTick)
    }

    private fun buildGamepadOverlay(): FrameLayout {
        val pad = FrameLayout(this).apply {
            setBackgroundColor(Color.argb(0, 0, 0, 0))
        }

        // D-pad в нижнем левом углу
        val dpad = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            gravity = Gravity.CENTER_HORIZONTAL
        }

        fun gameKey(label: String, keyCode: Int) = Button(this).apply {
            text = label
            setTextColor(Color.WHITE)
            background = roundedBg(Color.argb(0xCC, 0x11, 0x22, 0x33), 12)
            textSize = 14f
            isAllCaps = false
            setOnClickListener { client.keyControl(keyCode) }
        }

        val upRow = LinearLayout(this).apply { gravity = Gravity.CENTER }
        upRow.addView(gameKey("↑", 32), LinearLayout.LayoutParams(dp(54), dp(50)))
        val midRow = LinearLayout(this).apply { gravity = Gravity.CENTER }
        midRow.addView(gameKey("←", 22), LinearLayout.LayoutParams(dp(54), dp(50)).also { it.setMargins(0,0,dp(2),0) })
        midRow.addView(gameKey("↓", 6), LinearLayout.LayoutParams(dp(54), dp(50)).also { it.setMargins(dp(2),0,dp(2),0) })
        midRow.addView(gameKey("→", 28), LinearLayout.LayoutParams(dp(54), dp(50)).also { it.setMargins(dp(2),0,0,0) })
        dpad.addView(upRow)
        dpad.addView(vSpace(dp(2)))
        dpad.addView(midRow)

        val dpadLp = FrameLayout.LayoutParams(WRAP_CONTENT, WRAP_CONTENT, Gravity.BOTTOM or Gravity.START)
        dpadLp.setMargins(dp(16), 0, 0, dp(56))
        pad.addView(dpad, dpadLp)

        // Кнопки действий справа
        val actionCol = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            gravity = Gravity.END
        }
        val actRow1 = LinearLayout(this).apply { gravity = Gravity.CENTER }
        actRow1.addView(gameKey("Esc", 8), LinearLayout.LayoutParams(dp(54), dp(50)).also { it.setMargins(0,0,dp(4),0) })
        actRow1.addView(gameKey("Tab", 31), LinearLayout.LayoutParams(dp(54), dp(50)))
        val actRow2 = LinearLayout(this).apply { gravity = Gravity.CENTER }
        actRow2.addView(gameKey("Enter", 27), LinearLayout.LayoutParams(dp(68), dp(50)).also { it.setMargins(0,0,dp(4),0) })
        actRow2.addView(gameKey("Space", 27).apply {
            text = "Sp"
            setOnClickListener { client.keyText(" ") }
        }, LinearLayout.LayoutParams(dp(50), dp(50)))
        actionCol.addView(actRow1)
        actionCol.addView(vSpace(dp(2)))
        actionCol.addView(actRow2)

        val actLp = FrameLayout.LayoutParams(WRAP_CONTENT, WRAP_CONTENT, Gravity.BOTTOM or Gravity.END)
        actLp.setMargins(0, 0, dp(16), dp(56))
        pad.addView(actionCol, actLp)

        return pad
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
    private fun connect(
        id: String,
        password: String,
        statusLabel: TextView,
        touchpadOnly: Boolean = false,
        codec: String = selectedCodec(),
    ) {
        // Сообщить хосту размер экрана — pipeline сделает downscale до него.
        val dm = android.util.DisplayMetrics()
        @Suppress("DEPRECATION")
        windowManager.defaultDisplay.getRealMetrics(dm)
        client.setMaxResolution(dm.widthPixels, dm.heightPixels)

        val started = if (touchpadOnly) {
            client.startTouchpad(id, password, apiUrl(), idServer(), relayServer(), publicKey(), codec)
        } else {
            client.start(id, password, apiUrl(), idServer(), relayServer(), publicKey(), codec)
        }
        if (!started) {
            statusLabel.text = "Не удалось запустить сессию"
            statusLabel.setTextColor(Color.rgb(0xE3, 0x4B, 0x2F))
            return
        }
        rememberRecentSession(id)
        if (password.isNotEmpty()) savePassword(id, password)
        currentRemoteId = id
        activeCodec = codec
        if (touchpadOnly) {
            showTouchpadScreen()
        } else {
            showRemoteScreen()
        }
    }

    // ── Удалённый экран ───────────────────────────────────────────────────────
    private fun showRemoteScreen() {
        root.removeAllViews()
        kbVisible = false
        touchpadView = null

        val useHardware = activeCodec != "EVRTCK"

        // TextureView — resize via onVideoRectChanged doesn't destroy the surface,
        // so the decoder stays alive without an IDR gap (unlike SurfaceView).
        var textureView: TextureView? = null
        if (useHardware) {
            val tv = TextureView(this).apply {
                layoutParams = FrameLayout.LayoutParams(MATCH_PARENT, MATCH_PARENT)
                keepScreenOn = true
            }
            textureView = tv
            root.addView(tv)
        }

        val rv = RemoteView(this, client).apply {
            layoutParams = FrameLayout.LayoutParams(MATCH_PARENT, MATCH_PARENT)
            setNaturalScroll(prefs.getBoolean(PREF_NATURAL_SCROLL, true))
            if (useHardware) {
                isHardwareDecodeMode = true
                setBackgroundColor(Color.TRANSPARENT)
                onVideoRectChanged = { rect ->
                    textureView?.layoutParams = FrameLayout.LayoutParams(rect.width, rect.height).apply {
                        leftMargin = rect.x
                        topMargin = rect.y
                    }
                }
            }
        }
        remoteView = rv
        root.addView(rv)

        if (useHardware) {
            VideoDecoder.onDimensionsAvailable = { w, h ->
                handler.post { rv.setHardwareDimensions(w, h) }
            }
            textureView?.surfaceTextureListener = object : TextureView.SurfaceTextureListener {
                override fun onSurfaceTextureAvailable(st: SurfaceTexture, w: Int, h: Int) {
                    VideoDecoder.onDimensionsAvailable = { dw, dh ->
                        handler.post { rv.setHardwareDimensions(dw, dh) }
                    }
                    VideoDecoder.setSurface(Surface(st))
                }
                override fun onSurfaceTextureSizeChanged(st: SurfaceTexture, w: Int, h: Int) {}
                override fun onSurfaceTextureDestroyed(st: SurfaceTexture): Boolean {
                    VideoDecoder.releaseAll()
                    return true
                }
                override fun onSurfaceTextureUpdated(st: SurfaceTexture) {}
            }
        }

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

        val rotBtn = makeRotateBtn()

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
        toolbar.addView(rotBtn, LinearLayout.LayoutParams(0, dp(40), 1f).also {
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
            var hostNotified = false
            var authWatchTicks = 0
            override fun run() {
                val connected = client.isConnected()
                val status = client.status()
                // Auth watchdog: первые 15 сек (30 тиков) проверяем статус на ошибку пароля.
                // Тик 0 пропускаем — статус ещё "Connecting..." а не результат.
                if (!connected) {
                    authWatchTicks++
                    if (authWatchTicks in 2..30 && looksLikeAuthError(status)) {
                        handleAuthFailure(currentRemoteId)
                        return
                    }
                } else {
                    authWatchTicks = 99 // подключились — watchdog больше не нужен
                }

                val statusText = if (connected) "● $status" else status
                overlay.text = if (useHardware && connected) {
                    "$statusText  |  ${PerfStats.summary()}"
                } else {
                    statusText
                }
                if (connected && !hostNotified) {
                    hostNotified = true
                    notifySessionConnected(currentRemoteId)
                }
                handler.postDelayed(this, 500)
            }
        }
        handler.post(statusTick)
    }

    private fun showTouchpadScreen() {
        root.removeAllViews()
        kbVisible = false
        remoteView = null

        val tv = TouchpadView(this, client).apply {
            layoutParams = FrameLayout.LayoutParams(MATCH_PARENT, MATCH_PARENT)
            setNaturalScroll(prefs.getBoolean(PREF_NATURAL_SCROLL, true))
        }
        touchpadView = tv
        root.addView(tv)

        val proxy = buildKeyProxy()
        keyProxy = proxy
        root.addView(proxy, FrameLayout.LayoutParams(1, 1).also {
            it.gravity = Gravity.TOP or Gravity.START
        })

        val specRow = buildSpecialKeysRow()
        kbPanel = specRow

        val overlay = TextView(this).apply {
            setTextColor(Color.WHITE)
            setBackgroundColor(Color.argb(0xAA, 0, 0, 0))
            textSize = 11f
            setPadding(dp(10), dp(6), dp(10), dp(6))
        }
        root.addView(
            overlay,
            FrameLayout.LayoutParams(WRAP_CONTENT, WRAP_CONTENT, Gravity.TOP or Gravity.START),
        )

        val toolbar = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            setBackgroundColor(toolbarBg)
            gravity = Gravity.CENTER_VERTICAL
            setPadding(dp(6), dp(6), dp(6), dp(6))
        }

        val rcBtn = makeToolBtn("ПКМ", Color.rgb(0x44, 0x44, 0x55))
        rcBtn.setOnClickListener { tv.rightClickAtCursor() }

        val kbBtn = makeToolBtn("Клав.", Color.rgb(0x22, 0x44, 0x55))
        kbBtn.setOnClickListener { toggleKeyboard() }

        val centerBtn = makeToolBtn("Центр", Color.rgb(0x22, 0x44, 0x33))
        centerBtn.setOnClickListener { tv.centerCursor() }

        val rotBtn = makeRotateBtn()

        val discBtn = makeToolBtn("Выход", Color.rgb(0x66, 0x22, 0x22))
        discBtn.setOnClickListener { disconnect() }

        toolbar.addView(rcBtn, LinearLayout.LayoutParams(0, dp(40), 1f).also {
            it.setMargins(dp(3), 0, dp(3), 0)
        })
        toolbar.addView(kbBtn, LinearLayout.LayoutParams(0, dp(40), 1f).also {
            it.setMargins(dp(3), 0, dp(3), 0)
        })
        toolbar.addView(centerBtn, LinearLayout.LayoutParams(0, dp(40), 1f).also {
            it.setMargins(dp(3), 0, dp(3), 0)
        })
        toolbar.addView(rotBtn, LinearLayout.LayoutParams(0, dp(40), 1f).also {
            it.setMargins(dp(3), 0, dp(3), 0)
        })
        toolbar.addView(discBtn, LinearLayout.LayoutParams(0, dp(40), 1f).also {
            it.setMargins(dp(3), 0, dp(3), 0)
        })

        val bottomBar = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
        }
        bottomBar.addView(specRow, LinearLayout.LayoutParams(MATCH_PARENT, WRAP_CONTENT))
        bottomBar.addView(toolbar, LinearLayout.LayoutParams(MATCH_PARENT, WRAP_CONTENT))
        root.addView(
            bottomBar,
            FrameLayout.LayoutParams(MATCH_PARENT, WRAP_CONTENT, Gravity.BOTTOM),
        )

        val statusTick = object : Runnable {
            var hostNotified = false
            var authWatchTicks = 0
            override fun run() {
                val connected = client.isConnected()
                val status = client.status()
                if (!connected) {
                    authWatchTicks++
                    if (authWatchTicks in 2..30 && looksLikeAuthError(status)) {
                        handleAuthFailure(currentRemoteId)
                        return
                    }
                } else {
                    authWatchTicks = 99
                }
                tv.refreshRemoteSize()
                overlay.text = if (connected) "● $status" else status
                if (connected && !hostNotified) {
                    hostNotified = true
                    notifySessionConnected(currentRemoteId)
                }
                handler.postDelayed(this, 500)
            }
        }
        handler.post(statusTick)
    }

    private fun selectedCodec(): String = prefs.getString(PREF_CODEC, "EVRTCK") ?: "EVRTCK"

    private fun disconnect() {
        audioPlayer?.stop()
        audioPlayer = null
        remoteView?.stopRendering()
        VideoDecoder.releaseAll()
        client.stop()
        handler.removeCallbacksAndMessages(null)
        rightClickPending = false
        rightClickBtn = null
        rotateBtn = null
        remoteView = null
        touchpadView = null
        activeCodec = "EVRTCK"
        requestedOrientation = ActivityInfo.SCREEN_ORIENTATION_USER

        // Ждём ~700ms пока поток старой сессии обработает Close и закроет TCP-сокет к relay.
        // Без паузы повторное подключение приходит пока старый сокет жив — relay/хост отклоняют.
        root.removeAllViews()
        root.setBackgroundColor(brandBg)
        root.addView(
            TextView(this).apply {
                text = "Завершение сеанса…"
                setTextColor(textSoft)
                textSize = 17f
                gravity = Gravity.CENTER
            },
            FrameLayout.LayoutParams(MATCH_PARENT, MATCH_PARENT, Gravity.CENTER),
        )
        handler.postDelayed({ showConnectScreen() }, 700)
    }

    // ── Auth failure ──────────────────────────────────────────────────────────

    private fun looksLikeAuthError(status: String): Boolean {
        val s = status.lowercase()
        return s.contains("wrong") || s.contains("password") ||
               s.contains("denied") || s.contains("rejected") || s.contains("forbidden")
    }

    /** Вызывается когда в статусе обнаружена ошибка аутентификации.
     *  Останавливает сессию, удаляет устаревший пароль, показывает диалог. */
    private fun handleAuthFailure(id: String) {
        audioPlayer?.stop()
        audioPlayer = null
        remoteView?.stopRendering()
        VideoDecoder.releaseAll()
        client.stop()
        handler.removeCallbacksAndMessages(null)
        savePassword(id, "")   // устаревший пароль удалён
        remoteView = null
        touchpadView = null
        activeCodec = "EVRTCK"
        requestedOrientation = ActivityInfo.SCREEN_ORIENTATION_USER

        handler.postDelayed({
            showConnectScreen()
            showPasswordDialog(id)
        }, 400)
    }

    /** Диалог повторного ввода пароля после неудачной аутентификации. */
    private fun showPasswordDialog(id: String) {
        val overlay = FrameLayout(this).apply {
            setBackgroundColor(Color.argb(0xCC, 0, 0, 0))
            isClickable = true
            isFocusable = true
        }
        val card = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            background = roundedBg(cardBg, 22)
            setPadding(dp(20), dp(20), dp(20), dp(20))
        }

        card.addView(TextView(this).apply {
            text = "Пароль не подошёл"
            textSize = 19f
            typeface = Typeface.DEFAULT_BOLD
            setTextColor(Color.rgb(0xE3, 0x4B, 0x2F))
            gravity = Gravity.CENTER
        }, matchWrap())
        card.addView(vSpace(dp(10)))
        card.addView(TextView(this).apply {
            // Объясняем оба сценария — пользователь не знает почему хост нажал "Принять" но всё равно отказ
            text = "Два частых случая:\n\n" +
                   "• Хост нажал «Принять» — значит он работает без пароля. " +
                   "Нажмите «Без пароля» ниже — это правильный способ подключения к такому хосту.\n\n" +
                   "• Пароль изменился — введите новый пароль.\n\n" +
                   "Сохранённый пароль для ID $id удалён."
            setTextColor(textSoft)
            textSize = 13f
            setLineSpacing(0f, 1.25f)
        }, matchWrap())
        card.addView(vSpace(dp(16)))

        fun dismiss() { root.removeView(overlay) }

        // "Без пароля" — основная кнопка: именно это нужно когда хост в режиме "Принять"
        card.addView(makePrimaryButton("Без пароля (хост нажмёт «Принять»)") {
            dismiss()
            connect(id, "", TextView(this))
        }, LinearLayout.LayoutParams(MATCH_PARENT, dp(52)))
        card.addView(vSpace(dp(10)))

        // Разделитель "или"
        card.addView(TextView(this).apply {
            text = "— или введите новый пароль —"
            setTextColor(textSoft)
            textSize = 11f
            gravity = Gravity.CENTER
        }, matchWrap())
        card.addView(vSpace(dp(10)))

        val pwInput = makeInput("Новый пароль", true)
        card.addView(pwInput, matchWrap())
        card.addView(vSpace(dp(10)))
        card.addView(makeSecondaryButton("Подключиться с паролем") {
            val pw = pwInput.text.toString()
            dismiss()
            connect(id, pw, TextView(this))
        }, LinearLayout.LayoutParams(MATCH_PARENT, dp(46)))
        card.addView(vSpace(dp(8)))
        card.addView(makeSecondaryButton("Отмена") { dismiss() },
            LinearLayout.LayoutParams(MATCH_PARENT, dp(46)))

        val lp = FrameLayout.LayoutParams(MATCH_PARENT, WRAP_CONTENT, Gravity.CENTER).apply {
            val m = dp(24); setMargins(m, m, m, m)
        }
        overlay.addView(card, lp)
        root.addView(overlay, FrameLayout.LayoutParams(MATCH_PARENT, MATCH_PARENT))
        pwInput.requestFocus()
    }

    override fun onDestroy() {
        remoteView?.stopRendering()
        touchpadView = null
        client.stop()
        super.onDestroy()
    }

    // Вызывается системой при смене ориентации — Activity НЕ пересоздаётся (configChanges в манифесте).
    // MATCH_PARENT views автоматически перемеряются; обновляем только текст кнопки поворота.
    override fun onConfigurationChanged(newConfig: Configuration) {
        super.onConfigurationChanged(newConfig)
        val isLandscape = newConfig.orientation == Configuration.ORIENTATION_LANDSCAPE
        rotateBtn?.apply {
            text = if (isLandscape) "↕ Портрет" else "↔ Пейзаж"
            tag = isLandscape
        }
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
        row.addView(navButton("Игра", active == "game") { showGameScreen() })
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

    private fun loadLocalContacts(): List<AddressBookContact> {
        val raw = prefs.getString(PREF_AB_LOCAL_CONTACTS, "").orEmpty()
        if (raw.isBlank()) return emptyList()
        return try {
            val array = JSONArray(raw)
            val list = mutableListOf<AddressBookContact>()
            for (i in 0 until array.length()) {
                val item = array.optJSONObject(i) ?: continue
                val remoteId = item.optString("remote_id").filter { it.isDigit() }
                if (remoteId.isBlank()) continue
                list += AddressBookContact(
                    name = item.optString("name"),
                    remoteId = remoteId,
                    note = item.optString("note"),
                    os = "",
                    online = false,
                )
            }
            list
        } catch (_: Throwable) { emptyList() }
    }

    private fun saveLocalContacts(contacts: List<AddressBookContact>) {
        val array = JSONArray().also { a ->
            contacts.forEach { c ->
                a.put(JSONObject()
                    .put("name", c.name)
                    .put("remote_id", c.remoteId)
                    .put("note", c.note))
            }
        }
        prefs.edit().putString(PREF_AB_LOCAL_CONTACTS, array.toString()).apply()
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

    // Кнопка поворота экрана: тап переключает между пейзажем и портретом.
    // tag=true → сейчас пейзаж, следующий тап → портрет. И наоборот.
    private fun makeRotateBtn(): Button {
        val isLandscape = resources.configuration.orientation == Configuration.ORIENTATION_LANDSCAPE
        return makeToolBtn(
            if (isLandscape) "↕ Портрет" else "↔ Пейзаж",
            Color.rgb(0x22, 0x33, 0x44)
        ).apply {
            tag = isLandscape
            setOnClickListener { v ->
                val btn = v as Button
                val wasLandscape = btn.tag as Boolean
                if (wasLandscape) {
                    requestedOrientation = ActivityInfo.SCREEN_ORIENTATION_PORTRAIT
                    btn.text = "↔ Пейзаж"
                    btn.tag = false
                } else {
                    requestedOrientation = ActivityInfo.SCREEN_ORIENTATION_LANDSCAPE
                    btn.text = "↕ Портрет"
                    btn.tag = true
                }
            }
        }.also { rotateBtn = it }
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
