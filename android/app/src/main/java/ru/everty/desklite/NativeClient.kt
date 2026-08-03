package ru.everty.desklite

class NativeClient {
    data class RemoteGeometry(
        val x: Int,
        val y: Int,
        val width: Int,
        val height: Int,
    )

    private var handle: Long = 0

    fun start(
        id: String,
        password: String,
        apiUrl: String,
        idServer: String,
        relayServer: String,
        publicKey: String,
        codec: String = "EVRTCK",
    ): Boolean {
        handle = nativeStart(id, password, apiUrl, idServer, relayServer, publicKey, codec)
        return handle != 0L
    }

    fun startTouchpad(
        id: String,
        password: String,
        apiUrl: String,
        idServer: String,
        relayServer: String,
        publicKey: String,
        codec: String = "EVRTCK",
    ): Boolean {
        handle = nativeStartTouchpad(id, password, apiUrl, idServer, relayServer, publicKey, codec)
        return handle != 0L
    }

    fun frameSize(): Pair<Int, Int>? {
        if (handle == 0L) return null
        val packed = nativeFrameSize(handle)
        if (packed == 0L) return null
        return Pair((packed ushr 32).toInt(), (packed and 0xFFFFFFFFL).toInt())
    }

    fun remoteSize(): Pair<Int, Int>? {
        if (handle == 0L) return null
        val packed = nativeRemoteSize(handle)
        if (packed == 0L) return null
        return Pair((packed ushr 32).toInt(), (packed and 0xFFFFFFFFL).toInt())
    }

    fun remoteGeometry(): RemoteGeometry? {
        if (handle == 0L) return null
        val packedSize = nativeRemoteSize(handle)
        if (packedSize == 0L) return null
        val packedOrigin = nativeRemoteOrigin(handle)
        return RemoteGeometry(
            x = (packedOrigin shr 32).toInt(),
            y = packedOrigin.toInt(),
            width = (packedSize ushr 32).toInt(),
            height = (packedSize and 0xFFFFFFFFL).toInt(),
        )
    }

    fun pollFrame(out: IntArray): Pair<Int, Int>? {
        if (handle == 0L) return null
        val packed = nativePollFrame(handle, out)
        if (packed == 0L) return null
        return Pair((packed ushr 32).toInt(), (packed and 0xFFFFFFFFL).toInt())
    }

    /** action: 0=down 1=move 2=up, координаты удалённого экрана */
    fun touch(x: Int, y: Int, action: Int) {
        if (handle != 0L) nativeTouch(handle, x, y, action)
    }

    /** Правый клик в точке удалённого экрана */
    fun rightClick(x: Int, y: Int) {
        if (handle != 0L) nativeRightClick(handle, x, y)
    }

    /** Скролл колеса. deltaY > 0 — вниз, < 0 — вверх */
    fun scroll(x: Int, y: Int, deltaY: Int) {
        if (handle != 0L) nativeScroll(handle, x, y, deltaY)
    }

    /** Ввод текста на удалённом хосте */
    fun keyText(text: String) {
        if (handle != 0L) nativeKeyText(handle, text)
    }

    /** Установить системную громкость хоста, 0..100 (%). */
    fun setHostVolume(volume: Int) {
        if (handle != 0L) nativeSetHostVolume(handle, volume.coerceIn(0, 100))
    }

    /** Навигация браузера жестом (Alt+←/→). forward=false назад, true вперёд. */
    fun navigate(forward: Boolean) {
        if (handle != 0L) nativeNavigate(handle, forward)
    }

    /** Экспериментальная EVRT2: просит хост поднять отдельный тестовый поток. */
    fun startEvrt2Experiment() {
        if (handle != 0L) nativeStartEvrt2Experiment(handle)
    }

    /** Достать последний кадр EVRT2-эксперимента (отдельный от live-видео буфер). */
    fun pollEvrt2Frame(out: IntArray): Pair<Int, Int>? {
        if (handle == 0L) return null
        val packed = nativePollEvrt2Frame(handle, out)
        if (packed == 0L) return null
        return Pair((packed ushr 32).toInt(), (packed and 0xFFFFFFFFL).toInt())
    }

    /** Размер последнего кадра EVRT2-эксперимента, без изъятия данных. */
    fun evrt2FrameSize(): Pair<Int, Int>? {
        if (handle == 0L) return null
        val packed = nativeEvrt2FrameSize(handle)
        if (packed == 0L) return null
        return Pair((packed ushr 32).toInt(), (packed and 0xFFFFFFFFL).toInt())
    }

    /** Последний статус EVRT2-эксперимента (подключение, MODE_SWITCH, APF, счётчик кадров и т.д.). Пусто, если ничего не пришло. */
    fun evrt2Status(): String = if (handle == 0L) "" else nativeEvrt2Status(handle)

    /**
     * Управляющая клавиша. Коды (ControlKey):
     *   2=Backspace, 4=Ctrl, 5=Delete, 6=↓, 7=End, 8=Esc, 21=Home,
     *   22=←, 27=Enter, 28=→, 29=Shift, 31=Tab, 32=↑
     */
    fun keyControl(code: Int) {
        if (handle != 0L) nativeKeyControl(handle, code)
    }

    /** Ctrl+символ: зажать Ctrl, нажать ch, отпустить Ctrl */
    fun keyCtrl(ch: String) {
        if (handle != 0L) nativeKeyCtrl(handle, ch)
    }

    fun status(): String = if (handle == 0L) "—" else nativeStatus(handle)
    fun isConnected(): Boolean = handle != 0L && nativeIsConnected(handle)

    /** Сообщить хосту максимальное разрешение экрана клиента (до старта). */
    fun setMaxResolution(width: Int, height: Int) = nativeSetMaxResolution(width, height)

    fun stop() {
        if (handle != 0L) { nativeStop(handle); handle = 0 }
    }

    // ── Android-хост (тачпад): устройство принимает подключения ────────────────
    /** Запустить хост-режим. Возвращает true при успешном старте. */
    fun startTouchpadHost(
        localId: String,
        password: String,
        idServer: String,
        relayServer: String,
        publicKey: String,
        screenW: Int,
        screenH: Int,
    ): Boolean = nativeStartTouchpadHost(
        localId, password, idServer, relayServer, publicKey, screenW, screenH,
    )

    /** Остановить хост-режим. */
    fun stopHost() = nativeStopHost()

    /** Текущий статус хоста (последняя строка лога). */
    fun hostStatus(): String = nativeHostStatus()

    /** Достать PCM фрейм из аудио очереди (16-bit stereo LE). null = нет данных. */
    fun pollAudio(): ByteArray? = if (handle != 0L) nativePollAudio(handle) else null

    /** Sample rate аудио потока (из AudioConfig хоста). Default 48000. */
    fun audioSampleRate(): Int = nativeGetAudioSampleRate()

    /** Текущая глубина аудио очереди в фреймах (без изъятия). Для jitter buffer. */
    fun audioQueueDepth(): Int = nativeAudioQueueDepth()

    private external fun nativeStart(
        id: String,
        password: String,
        apiUrl: String,
        idServer: String,
        relayServer: String,
        publicKey: String,
        codec: String,
    ): Long
    private external fun nativeStartTouchpad(
        id: String,
        password: String,
        apiUrl: String,
        idServer: String,
        relayServer: String,
        publicKey: String,
        codec: String,
    ): Long
    private external fun nativeFrameSize(handle: Long): Long
    private external fun nativeRemoteSize(handle: Long): Long
    private external fun nativeRemoteOrigin(handle: Long): Long
    private external fun nativePollFrame(handle: Long, out: IntArray): Long
    private external fun nativeTouch(handle: Long, x: Int, y: Int, action: Int)
    private external fun nativeRightClick(handle: Long, x: Int, y: Int)
    private external fun nativeScroll(handle: Long, x: Int, y: Int, deltaY: Int)
    private external fun nativeKeyText(handle: Long, text: String)
    private external fun nativeSetHostVolume(handle: Long, volume: Int)
    private external fun nativeNavigate(handle: Long, forward: Boolean)
    private external fun nativeStartEvrt2Experiment(handle: Long)
    private external fun nativeEvrt2FrameSize(handle: Long): Long
    private external fun nativePollEvrt2Frame(handle: Long, out: IntArray): Long
    private external fun nativeEvrt2Status(handle: Long): String
    private external fun nativeKeyControl(handle: Long, code: Int)
    private external fun nativeKeyCtrl(handle: Long, ch: String)
    private external fun nativeStatus(handle: Long): String
    private external fun nativeIsConnected(handle: Long): Boolean
    private external fun nativeStop(handle: Long)
    private external fun nativeSetMaxResolution(width: Int, height: Int)
    private external fun nativePollAudio(handle: Long): ByteArray?
    private external fun nativeGetAudioSampleRate(): Int
    private external fun nativeAudioQueueDepth(): Int
    private external fun nativeStartTouchpadHost(
        localId: String,
        password: String,
        idServer: String,
        relayServer: String,
        publicKey: String,
        screenW: Int,
        screenH: Int,
    ): Boolean
    private external fun nativeStopHost()
    private external fun nativeHostStatus(): String

    companion object {
        init { System.loadLibrary("evertydesk_core") }
    }
}
