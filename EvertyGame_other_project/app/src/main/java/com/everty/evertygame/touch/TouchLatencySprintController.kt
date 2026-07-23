package com.everty.evertygame.touch

import android.graphics.Rect
import android.os.SystemClock
import java.util.concurrent.atomic.AtomicLong

object TouchLatencySprintController {
    enum class PulseKind(
        val durationMs: Long,
        val intensity: Int,
    ) {
        TAP(durationMs = 180L, intensity = 1),
        LONG_PRESS(durationMs = 300L, intensity = 2),
        SCROLL(durationMs = 260L, intensity = 2),
        GESTURE(durationMs = 320L, intensity = 3),
    }

    data class PulseSnapshot(
        val untilElapsedRealtimeMs: Long,
        val intensity: Int,
        val generation: Long,
    )

    data class RoiSnapshot(
        val rect: Rect,
        val pulseKind: PulseKind,
        val activeUntilElapsedRealtimeMs: Long,
        val warmUntilElapsedRealtimeMs: Long,
        val generation: Long,
    ) {
        val isActive: Boolean
            get() = warmUntilElapsedRealtimeMs > SystemClock.elapsedRealtime()
    }

    private const val MIN_EVENT_GAP_MS = 16L
    private const val ROI_WARM_EXTRA_MS = 120L

    private val sprintUntilElapsedRealtimeMs = AtomicLong(0L)
    private val lastTriggerElapsedRealtimeMs = AtomicLong(0L)
    private val currentIntensity = AtomicLong(0L)
    private val pulseGeneration = AtomicLong(0L)
    private val roiSync = Any()
    private var lastPulseKind = PulseKind.TAP
    private var lastBounds: Rect? = null
    private var roiActiveUntilElapsedRealtimeMs = 0L
    private var roiWarmUntilElapsedRealtimeMs = 0L

    fun trigger(kind: PulseKind = PulseKind.TAP, boundsInScreen: Rect? = null) {
        val now = SystemClock.elapsedRealtime()
        val lastTrigger = lastTriggerElapsedRealtimeMs.get()
        if (now - lastTrigger < MIN_EVENT_GAP_MS) {
            return
        }
        lastTriggerElapsedRealtimeMs.set(now)

        val clampedDurationMs = kind.durationMs.coerceIn(120L, 260L)
        val targetUntilMs = now + clampedDurationMs
        while (true) {
            val currentUntilMs = sprintUntilElapsedRealtimeMs.get()
            if (currentUntilMs >= targetUntilMs) {
                break
            }
            if (sprintUntilElapsedRealtimeMs.compareAndSet(currentUntilMs, targetUntilMs)) {
                break
            }
        }

        while (true) {
            val current = currentIntensity.get()
            if (current >= kind.intensity.toLong()) {
                break
            }
            if (currentIntensity.compareAndSet(current, kind.intensity.toLong())) {
                break
            }
        }

        synchronized(roiSync) {
            lastPulseKind = kind
            if (boundsInScreen != null && !boundsInScreen.isEmpty) {
                lastBounds = Rect(boundsInScreen)
            }
            roiActiveUntilElapsedRealtimeMs = targetUntilMs
            roiWarmUntilElapsedRealtimeMs = targetUntilMs + ROI_WARM_EXTRA_MS
        }

        pulseGeneration.incrementAndGet()
    }

    fun currentPulseSnapshot(now: Long = SystemClock.elapsedRealtime()): PulseSnapshot {
        val untilMs = sprintUntilElapsedRealtimeMs.get()
        if (untilMs <= now) {
            currentIntensity.set(0L)
            return PulseSnapshot(
                untilElapsedRealtimeMs = untilMs,
                intensity = 0,
                generation = pulseGeneration.get(),
            )
        }

        return PulseSnapshot(
            untilElapsedRealtimeMs = untilMs,
            intensity = currentIntensity.get().toInt().coerceAtLeast(1),
            generation = pulseGeneration.get(),
        )
    }

    fun currentSprintUntilElapsedRealtimeMs(): Long = sprintUntilElapsedRealtimeMs.get()

    fun isSprintActive(now: Long = SystemClock.elapsedRealtime()): Boolean {
        return currentSprintUntilElapsedRealtimeMs() > now
    }

    fun currentRoiSnapshot(
        screenWidth: Int,
        screenHeight: Int,
        now: Long = SystemClock.elapsedRealtime(),
    ): RoiSnapshot? {
        if (screenWidth <= 0 || screenHeight <= 0) {
            return null
        }

        synchronized(roiSync) {
            if (roiWarmUntilElapsedRealtimeMs <= now) {
                return null
            }

            val baseRect = when {
                lastBounds != null && !lastBounds!!.isEmpty -> Rect(lastBounds)
                else -> centeredFallbackRect(screenWidth, screenHeight)
            }
            val expanded = expandAndSquareRect(baseRect, screenWidth, screenHeight)
            return RoiSnapshot(
                rect = expanded,
                pulseKind = lastPulseKind,
                activeUntilElapsedRealtimeMs = roiActiveUntilElapsedRealtimeMs,
                warmUntilElapsedRealtimeMs = roiWarmUntilElapsedRealtimeMs,
                generation = pulseGeneration.get(),
            )
        }
    }

    fun clear() {
        sprintUntilElapsedRealtimeMs.set(0L)
        lastTriggerElapsedRealtimeMs.set(0L)
        currentIntensity.set(0L)
        pulseGeneration.set(0L)
        synchronized(roiSync) {
            lastPulseKind = PulseKind.TAP
            lastBounds = null
            roiActiveUntilElapsedRealtimeMs = 0L
            roiWarmUntilElapsedRealtimeMs = 0L
        }
    }

    private fun centeredFallbackRect(screenWidth: Int, screenHeight: Int): Rect {
        val size = (minOf(screenWidth, screenHeight) * 0.35f).toInt().coerceAtLeast(192)
        val left = ((screenWidth - size) / 2).coerceAtLeast(0)
        val top = ((screenHeight - size) / 2).coerceAtLeast(0)
        return Rect(left, top, (left + size).coerceAtMost(screenWidth), (top + size).coerceAtMost(screenHeight))
    }

    private fun expandAndSquareRect(baseRect: Rect, screenWidth: Int, screenHeight: Int): Rect {
        val width = baseRect.width().coerceAtLeast(1)
        val height = baseRect.height().coerceAtLeast(1)
        val expansion = (maxOf(width, height) * 0.12f).toInt().coerceAtLeast(12)
        val expanded = Rect(
            (baseRect.left - expansion).coerceAtLeast(0),
            (baseRect.top - expansion).coerceAtLeast(0),
            (baseRect.right + expansion).coerceAtMost(screenWidth),
            (baseRect.bottom + expansion).coerceAtMost(screenHeight),
        )
        val squareSize = maxOf(expanded.width(), expanded.height()).coerceAtLeast(192)
        val centerX = expanded.centerX()
        val centerY = expanded.centerY()
        val halfSize = squareSize / 2

        var left = centerX - halfSize
        var top = centerY - halfSize
        var right = left + squareSize
        var bottom = top + squareSize

        if (left < 0) {
            right -= left
            left = 0
        }
        if (top < 0) {
            bottom -= top
            top = 0
        }
        if (right > screenWidth) {
            val shift = right - screenWidth
            left -= shift
            right = screenWidth
        }
        if (bottom > screenHeight) {
            val shift = bottom - screenHeight
            top -= shift
            bottom = screenHeight
        }

        left = left.coerceAtLeast(0)
        top = top.coerceAtLeast(0)
        right = right.coerceIn(left + 1, screenWidth)
        bottom = bottom.coerceIn(top + 1, screenHeight)
        return Rect(left, top, right, bottom)
    }
}
