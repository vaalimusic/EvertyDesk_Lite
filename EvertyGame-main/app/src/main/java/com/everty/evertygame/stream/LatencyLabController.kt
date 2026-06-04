package com.everty.evertygame.stream

object LatencyLabController {
    data class PendingVisiblePulse(
        val pulseId: Int,
        val source: String,
        val visibleFramePresentationTimeUs: Long,
        val tapToUiMs: Int,
    )

    data class StreamPulseMarker(
        val pulseId: Int,
        val source: String,
        val presentationTimeUs: Long,
        val tapToUiMs: Int,
        val senderPipelineMs: Int,
        val approxSenderMs: Int,
    )

    private val sync = Any()
    private var nextPulseId = 0
    private var pendingVisiblePulse: PendingVisiblePulse? = null

    fun triggerPulse(source: String): Int {
        synchronized(sync) {
            nextPulseId += 1
            return nextPulseId
        }
    }

    fun markPulseVisible(
        pulseId: Int,
        source: String,
        framePresentationTimeUs: Long,
        tapToUiMs: Int,
    ) {
        if (pulseId <= 0 || framePresentationTimeUs <= 0L) {
            return
        }

        synchronized(sync) {
            if (pulseId != nextPulseId) {
                return
            }

            pendingVisiblePulse = PendingVisiblePulse(
                pulseId = pulseId,
                source = source,
                visibleFramePresentationTimeUs = framePresentationTimeUs,
                tapToUiMs = tapToUiMs.coerceAtLeast(0),
            )
        }
    }

    fun consumeMarkerForFrame(framePresentationTimeUs: Long, senderPipelineMs: Int): StreamPulseMarker? {
        if (framePresentationTimeUs <= 0L) {
            return null
        }

        synchronized(sync) {
            val pending = pendingVisiblePulse ?: return null
            if (framePresentationTimeUs < pending.visibleFramePresentationTimeUs) {
                return null
            }

            pendingVisiblePulse = null
            val pipelineMs = senderPipelineMs.coerceAtLeast(0)
            return StreamPulseMarker(
                pulseId = pending.pulseId,
                source = pending.source,
                presentationTimeUs = framePresentationTimeUs,
                tapToUiMs = pending.tapToUiMs,
                senderPipelineMs = pipelineMs,
                approxSenderMs = pending.tapToUiMs + pipelineMs,
            )
        }
    }

    fun clear() {
        synchronized(sync) {
            nextPulseId = 0
            pendingVisiblePulse = null
        }
    }
}
