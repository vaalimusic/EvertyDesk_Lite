package com.everty.evertygame.stream

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.media.projection.MediaProjectionManager
import android.os.Build
import android.os.IBinder
import androidx.core.app.NotificationCompat
import androidx.core.content.ContextCompat
import com.everty.evertygame.MainActivity
import com.everty.evertygame.R
import com.everty.evertygame.touch.TouchLatencySprintController

class StreamingService : Service() {
    private var streamer: ScreenCaptureStreamer? = null
    private var finishing = false

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_STOP -> stopSession("Streaming stopped", isError = false)
            ACTION_START -> startSession(intent)
        }
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        streamer?.stop()
        streamer = null
        LatencyLabController.clear()
        if (!finishing) {
            StreamingSessionStore.markStopped("Foreground service stopped")
        }
        stopForeground(STOP_FOREGROUND_REMOVE)
        super.onDestroy()
    }

    private fun startSession(intent: Intent) {
        val config = extractConfig(intent)
        val resultCode = intent.getIntExtra(EXTRA_RESULT_CODE, Int.MIN_VALUE)
        val projectionData = intent.parcelableIntentExtra(EXTRA_PROJECTION_DATA)
        val projectionManager = getSystemService(MediaProjectionManager::class.java)

        if (config == null || resultCode == Int.MIN_VALUE || projectionData == null || projectionManager == null) {
            stopSession("Missing stream start parameters", isError = true)
            return
        }

        finishing = false
        streamer?.stop()
        streamer = null
        TouchLatencySprintController.clear()
        LatencyLabController.clear()

        StreamingSessionStore.markStarting(config)
        ensureNotificationChannel()
        startForegroundCompat(buildNotification("Preparing sender session"))

        streamer = ScreenCaptureStreamer(
            context = this,
            config = config,
            projectionManager = projectionManager,
            resultCode = resultCode,
            projectionData = projectionData,
            onStreamingStarted = { resolutionLabel ->
                StreamingSessionStore.markStreaming(config, resolutionLabel)
                updateNotification("Streaming ${config.codec.uiName} via ${config.transport.uiName} to ${config.host}:${config.port}")
            },
            onMetricsUpdated = { metrics ->
                StreamingSessionStore.updateMetrics(metrics)
            },
            onFatalError = { message ->
                stopSession(message, isError = true)
            },
        ).also { it.start() }
    }

    private fun stopSession(message: String, isError: Boolean) {
        if (finishing) {
            return
        }

        finishing = true
        streamer?.stop()
        streamer = null

        if (isError) {
            StreamingSessionStore.markError(message)
        } else {
            StreamingSessionStore.markStopped(message)
        }
        TouchLatencySprintController.clear()
        LatencyLabController.clear()

        stopForeground(STOP_FOREGROUND_REMOVE)
        stopSelf()
    }

    private fun startForegroundCompat(notification: Notification) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            startForeground(
                NOTIFICATION_ID,
                notification,
                ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION,
            )
        } else {
            startForeground(NOTIFICATION_ID, notification)
        }
    }

    private fun updateNotification(content: String) {
        val manager = getSystemService(NotificationManager::class.java)
        manager.notify(NOTIFICATION_ID, buildNotification(content))
    }

    private fun buildNotification(content: String): Notification {
        val openAppIntent = PendingIntent.getActivity(
            this,
            1,
            Intent(this, MainActivity::class.java).apply {
                flags = Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP
            },
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )

        val stopIntent = PendingIntent.getService(
            this,
            2,
            Intent(this, StreamingService::class.java).setAction(ACTION_STOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )

        return NotificationCompat.Builder(this, CHANNEL_ID)
            .setSmallIcon(R.mipmap.ic_launcher)
            .setContentTitle("Everty Sender")
            .setContentText(content)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setContentIntent(openAppIntent)
            .addAction(0, "Stop", stopIntent)
            .build()
    }

    private fun ensureNotificationChannel() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) {
            return
        }

        val manager = getSystemService(NotificationManager::class.java)
        val channel = NotificationChannel(
            CHANNEL_ID,
            "Everty Streaming",
            NotificationManager.IMPORTANCE_LOW,
        ).apply {
            description = "Foreground notification for screen streaming"
            setShowBadge(false)
        }

        manager.createNotificationChannel(channel)
    }

    private fun extractConfig(intent: Intent): StreamConfig? {
        val host = intent.getStringExtra(EXTRA_HOST)?.trim().orEmpty()
        val port = intent.getIntExtra(EXTRA_PORT, -1)
        val presetName = intent.getStringExtra(EXTRA_PRESET_NAME)
        val codecName = intent.getStringExtra(EXTRA_CODEC_NAME)
        val targetFps = intent.getIntExtra(EXTRA_TARGET_FPS, -1)
        val targetBitrateBps = intent.getIntExtra(EXTRA_TARGET_BITRATE_BPS, -1)
        val audioEnabled = intent.getBooleanExtra(EXTRA_AUDIO_ENABLED, false)
        val adaptationModeName = intent.getStringExtra(EXTRA_ADAPTATION_MODE)
        val transportName = intent.getStringExtra(EXTRA_TRANSPORT)
        val touchLatencySprintEnabled = intent.getBooleanExtra(EXTRA_TOUCH_LATENCY_SPRINT_ENABLED, true)
        val gamepadBoostEnabled = intent.getBooleanExtra(EXTRA_GAMEPAD_BOOST_ENABLED, false)
        val adaptiveRoiSplitStreamEnabled = intent.getBooleanExtra(EXTRA_ADAPTIVE_ROI_SPLIT_STREAM_ENABLED, true)
        if (
            host.isBlank() ||
            port !in 1..65535 ||
            presetName.isNullOrBlank() ||
            codecName.isNullOrBlank() ||
            targetFps !in 24..120 ||
            targetBitrateBps !in 1_000_000..100_000_000 ||
            adaptationModeName.isNullOrBlank() ||
            transportName.isNullOrBlank()
        ) {
            return null
        }

        return runCatching {
            StreamConfig(
                host = host,
                port = port,
                transport = StreamTransport.valueOf(transportName),
                preset = QualityPreset.valueOf(presetName),
                targetFps = targetFps,
                targetBitrateBps = targetBitrateBps,
                codec = VideoCodec.valueOf(codecName),
                audioEnabled = audioEnabled,
                adaptationMode = AdaptationMode.valueOf(adaptationModeName),
                touchLatencySprintEnabled = touchLatencySprintEnabled,
                gamepadBoostEnabled = gamepadBoostEnabled,
                adaptiveRoiSplitStreamEnabled = adaptiveRoiSplitStreamEnabled,
            )
        }.getOrNull()
    }

    private fun Intent.parcelableIntentExtra(key: String): Intent? {
        return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            getParcelableExtra(key, Intent::class.java)
        } else {
            @Suppress("DEPRECATION")
            getParcelableExtra(key)
        }
    }

    companion object {
        private const val ACTION_START = "com.everty.evertygame.action.START_STREAMING"
        private const val ACTION_STOP = "com.everty.evertygame.action.STOP_STREAMING"
        private const val EXTRA_RESULT_CODE = "extra_result_code"
        private const val EXTRA_PROJECTION_DATA = "extra_projection_data"
        private const val EXTRA_HOST = "extra_host"
        private const val EXTRA_PORT = "extra_port"
        private const val EXTRA_PRESET_NAME = "extra_preset_name"
        private const val EXTRA_TARGET_FPS = "extra_target_fps"
        private const val EXTRA_TARGET_BITRATE_BPS = "extra_target_bitrate_bps"
        private const val EXTRA_CODEC_NAME = "extra_codec_name"
        private const val EXTRA_AUDIO_ENABLED = "extra_audio_enabled"
        private const val EXTRA_ADAPTATION_MODE = "extra_adaptation_mode"
        private const val EXTRA_TRANSPORT = "extra_transport"
        private const val EXTRA_TOUCH_LATENCY_SPRINT_ENABLED = "extra_touch_latency_sprint_enabled"
        private const val EXTRA_GAMEPAD_BOOST_ENABLED = "extra_gamepad_boost_enabled"
        private const val EXTRA_ADAPTIVE_ROI_SPLIT_STREAM_ENABLED = "extra_adaptive_roi_split_stream_enabled"
        private const val CHANNEL_ID = "everty_streaming"
        private const val NOTIFICATION_ID = 501

        fun start(
            context: Context,
            resultCode: Int,
            projectionData: Intent,
            config: StreamConfig,
        ) {
            val serviceIntent = Intent(context, StreamingService::class.java).apply {
                action = ACTION_START
                putExtra(EXTRA_RESULT_CODE, resultCode)
                putExtra(EXTRA_PROJECTION_DATA, projectionData)
                putExtra(EXTRA_HOST, config.host)
                putExtra(EXTRA_PORT, config.port)
                putExtra(EXTRA_TRANSPORT, config.transport.name)
                putExtra(EXTRA_PRESET_NAME, config.preset.name)
                putExtra(EXTRA_TARGET_FPS, config.targetFps)
                putExtra(EXTRA_TARGET_BITRATE_BPS, config.targetBitrateBps)
                putExtra(EXTRA_CODEC_NAME, config.codec.name)
                putExtra(EXTRA_AUDIO_ENABLED, config.audioEnabled)
                putExtra(EXTRA_ADAPTATION_MODE, config.adaptationMode.name)
                putExtra(EXTRA_TOUCH_LATENCY_SPRINT_ENABLED, config.touchLatencySprintEnabled)
                putExtra(EXTRA_GAMEPAD_BOOST_ENABLED, config.gamepadBoostEnabled)
                putExtra(EXTRA_ADAPTIVE_ROI_SPLIT_STREAM_ENABLED, config.adaptiveRoiSplitStreamEnabled)
            }
            ContextCompat.startForegroundService(context, serviceIntent)
        }

        fun stop(context: Context) {
            val serviceIntent = Intent(context, StreamingService::class.java).apply {
                action = ACTION_STOP
            }
            context.startService(serviceIntent)
        }
    }
}
