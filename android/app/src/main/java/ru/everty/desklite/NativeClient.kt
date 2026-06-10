package ru.everty.desklite

/**
 * JNI-мост к Rust-ядру EvertyDesk (libevertydesk_core.so).
 *
 * Имена функций совпадают с `Java_ru_everty_desklite_NativeClient_*` в
 * src/android_ffi.rs. Сессия живёт в Rust, здесь только тонкая обёртка.
 */
class NativeClient {
    private var handle: Long = 0

    /** Запустить подключение. Возвращает true если поток сессии стартовал. */
    fun start(id: String, password: String): Boolean {
        handle = nativeStart(id, password)
        return handle != 0L
    }

    /** Размер последнего кадра: Pair(width, height) или null если кадра нет. */
    fun frameSize(): Pair<Int, Int>? {
        if (handle == 0L) return null
        val packed = nativeFrameSize(handle)
        if (packed == 0L) return null
        val w = (packed ushr 32).toInt()
        val h = (packed and 0xFFFFFFFFL).toInt()
        return Pair(w, h)
    }

    /** Скопировать последний кадр в `out` (ARGB). Возвращает Pair(w,h) или null. */
    fun pollFrame(out: IntArray): Pair<Int, Int>? {
        if (handle == 0L) return null
        val packed = nativePollFrame(handle, out)
        if (packed == 0L) return null
        val w = (packed ushr 32).toInt()
        val h = (packed and 0xFFFFFFFFL).toInt()
        return Pair(w, h)
    }

    /** Тач → mouse. action: 0=down, 1=move, 2=up. Координаты удалённого экрана. */
    fun touch(x: Int, y: Int, action: Int) {
        if (handle != 0L) nativeTouch(handle, x, y, action)
    }

    fun status(): String = if (handle == 0L) "—" else nativeStatus(handle)

    fun isConnected(): Boolean = handle != 0L && nativeIsConnected(handle)

    fun stop() {
        if (handle != 0L) {
            nativeStop(handle)
            handle = 0
        }
    }

    // ── native ──────────────────────────────────────────────────────────────
    private external fun nativeStart(id: String, password: String): Long
    private external fun nativeFrameSize(handle: Long): Long
    private external fun nativePollFrame(handle: Long, out: IntArray): Long
    private external fun nativeTouch(handle: Long, x: Int, y: Int, action: Int)
    private external fun nativeStatus(handle: Long): String
    private external fun nativeIsConnected(handle: Long): Boolean
    private external fun nativeStop(handle: Long)

    companion object {
        init {
            System.loadLibrary("evertydesk_core")
        }
    }
}
