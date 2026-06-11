package ru.everty.desklite

class NativeClient {
    private var handle: Long = 0

    fun start(id: String, password: String): Boolean {
        handle = nativeStart(id, password)
        return handle != 0L
    }

    fun frameSize(): Pair<Int, Int>? {
        if (handle == 0L) return null
        val packed = nativeFrameSize(handle)
        if (packed == 0L) return null
        return Pair((packed ushr 32).toInt(), (packed and 0xFFFFFFFFL).toInt())
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

    fun status(): String = if (handle == 0L) "—" else nativeStatus(handle)
    fun isConnected(): Boolean = handle != 0L && nativeIsConnected(handle)

    fun stop() {
        if (handle != 0L) { nativeStop(handle); handle = 0 }
    }

    private external fun nativeStart(id: String, password: String): Long
    private external fun nativeFrameSize(handle: Long): Long
    private external fun nativePollFrame(handle: Long, out: IntArray): Long
    private external fun nativeTouch(handle: Long, x: Int, y: Int, action: Int)
    private external fun nativeRightClick(handle: Long, x: Int, y: Int)
    private external fun nativeScroll(handle: Long, x: Int, y: Int, deltaY: Int)
    private external fun nativeStatus(handle: Long): String
    private external fun nativeIsConnected(handle: Long): Boolean
    private external fun nativeStop(handle: Long)

    companion object {
        init { System.loadLibrary("evertydesk_core") }
    }
}
