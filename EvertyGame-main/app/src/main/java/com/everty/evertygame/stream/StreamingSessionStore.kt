package com.everty.evertygame.stream

import android.os.Handler
import android.os.Looper
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue

object StreamingSessionStore {
    private val mainHandler = Handler(Looper.getMainLooper())

    var uiState by mutableStateOf(StreamUiState())
        private set

    fun markPermissionRequested(config: StreamConfig) {
        mutate {
            it.copy(
                phase = StreamPhase.REQUESTING_PERMISSION,
                status = "Waiting for screen capture permission",
                activeEndpoint = "${config.transport.uiName} -> ${config.host}:${config.port}",
                activePreset = config.preset,
                activeCodec = config.codec,
                lastError = null,
            )
        }
    }

    fun markStarting(config: StreamConfig) {
        mutate {
            it.copy(
                phase = StreamPhase.STARTING,
                status = "Starting encoder and ${config.transport.uiName}",
                activeEndpoint = "${config.transport.uiName} -> ${config.host}:${config.port}",
                activePreset = config.preset,
                activeCodec = config.codec,
                metrics = it.metrics.copy(
                    fps = 0,
                    bitrateKbps = 0,
                    pipelineLatencyMs = 0,
                    framesSent = 0,
                    packetsSent = 0,
                    droppedFrames = 0,
                    resolutionLabel = "-",
                ),
                lastError = null,
            )
        }
    }

    fun markStreaming(config: StreamConfig, resolutionLabel: String) {
        mutate {
            it.copy(
                phase = StreamPhase.STREAMING,
                status = "Streaming",
                activeEndpoint = "${config.transport.uiName} -> ${config.host}:${config.port}",
                activePreset = config.preset,
                activeCodec = config.codec,
                metrics = it.metrics.copy(resolutionLabel = resolutionLabel),
                lastError = null,
            )
        }
    }

    fun updateMetrics(metrics: StreamMetrics) {
        mutate { current ->
            current.copy(metrics = metrics)
        }
    }

    fun markStopped(message: String = "Streaming stopped") {
        mutate {
            StreamUiState(
                phase = StreamPhase.IDLE,
                status = message,
                lastError = it.lastError,
            )
        }
    }

    fun markError(message: String) {
        mutate {
            it.copy(
                phase = StreamPhase.ERROR,
                status = message,
                lastError = message,
            )
        }
    }

    private fun mutate(transform: (StreamUiState) -> StreamUiState) {
        if (Looper.myLooper() == Looper.getMainLooper()) {
            uiState = transform(uiState)
        } else {
            mainHandler.post {
                uiState = transform(uiState)
            }
        }
    }
}
