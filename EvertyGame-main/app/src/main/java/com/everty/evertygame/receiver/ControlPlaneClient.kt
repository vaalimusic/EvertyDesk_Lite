package com.everty.evertygame.receiver

import android.content.Context
import android.os.Build
import android.util.Log
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject
import java.io.BufferedReader
import java.io.InputStreamReader
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.HttpURLConnection
import java.net.InetAddress
import java.net.URL
import java.nio.charset.StandardCharsets

internal data class ControlPlaneHostSummary(
    val hostId: String,
    val hostCode: String,
    val displayName: String,
    val region: String,
    val online: Boolean,
    val availability: String,
    val activeSessionId: String?,
    val supportsHevc: Boolean,
    val supportsAudio: Boolean,
    val supportsGamepad: Boolean,
    val pricePerHour: Double? = null,
    val currency: String? = null,
    val description: String? = null,
)

internal data class ControlPlaneSessionLease(
    val sessionId: String,
    val sessionToken: String,
    val hostId: String,
    val hostDisplayName: String,
    val status: String,
    val routeKind: String,
    val routeState: String,
    val routeVersion: Int,
    val sessionHealth: String,
    val sessionHealthReason: String,
    val routeActionHint: String,
    val routeActionReason: String,
    val routeFallbackReadyDurationSeconds: Int,
    val routeRecoveryReadyDurationSeconds: Int,
    val recommendedSyncDelaySeconds: Int,
    val transportLossLevel: String,
    val transportAnomalyKind: String,
    val transportAnomalyReason: String,
    val transportAnomalyConfidence: String,
    val receiverTelemetryAgeSeconds: Int,
    val senderTelemetryAgeSeconds: Int,
    val lastRouteActionKind: String?,
    val lastRouteActionReason: String?,
    val lastRouteActionActor: String?,
    val lastRouteActionUtc: String?,
    val routeRecoveryCount: Int,
    val routeRecoveryCooldownSeconds: Int,
    val routeFallbackCount: Int,
    val routeFallbackCooldownSeconds: Int,
    val codecPreference: String?,
    val relayAddress: String?,
    val relayPort: Int?,
    val relayRegion: String?,
    val probeAddress: String?,
    val probePort: Int?,
    val probeToken: String,
    val natStatus: String,
    val hostNatProbeAgeSeconds: Int,
    val clientNatProbeAgeSeconds: Int,
    val natProbeFresh: Boolean,
    val receiverAddress: String?,
    val receiverPort: Int?,
)

internal data class ControlPlaneConnectInstructions(
    val sessionId: String,
    val hostId: String,
    val hostDisplayName: String,
    val status: String,
    val routeKind: String,
    val routeState: String,
    val routeVersion: Int,
    val sessionHealth: String,
    val sessionHealthReason: String,
    val routeActionHint: String,
    val routeActionReason: String,
    val routeFallbackReadyDurationSeconds: Int,
    val routeRecoveryReadyDurationSeconds: Int,
    val recommendedSyncDelaySeconds: Int,
    val transportLossLevel: String,
    val transportAnomalyKind: String,
    val transportAnomalyReason: String,
    val transportAnomalyConfidence: String,
    val receiverTelemetryAgeSeconds: Int,
    val senderTelemetryAgeSeconds: Int,
    val lastRouteActionKind: String?,
    val lastRouteActionReason: String?,
    val lastRouteActionActor: String?,
    val lastRouteActionUtc: String?,
    val routeRecoveryCount: Int,
    val routeRecoveryCooldownSeconds: Int,
    val routeFallbackCount: Int,
    val routeFallbackCooldownSeconds: Int,
    val streamHost: String,
    val streamPort: Int,
    val relayHost: String?,
    val relayPort: Int?,
    val relayRegion: String?,
    val probeHost: String?,
    val probePort: Int?,
    val probeToken: String,
    val natStatus: String,
    val receiverRegistered: Boolean,
    val hostReady: Boolean,
    val hostNatProbeAgeSeconds: Int,
    val clientNatProbeAgeSeconds: Int,
    val natProbeFresh: Boolean,
)

internal data class ControlPlaneRoutePolicy(
    val sessionId: String,
    val hostId: String,
    val routeKind: String,
    val routeState: String,
    val routeVersion: Int,
    val sessionHealth: String,
    val sessionHealthReason: String,
    val routeActionHint: String,
    val routeActionReason: String,
    val recommendedSyncDelaySeconds: Int,
    val transportLossLevel: String,
    val transportAnomalyKind: String,
    val transportAnomalyReason: String,
    val transportAnomalyConfidence: String,
    val actionableAnomaly: Boolean,
    val highConfidenceAnomaly: Boolean,
    val fallbackWarmupSeconds: Int,
    val fallbackReadyDurationSeconds: Int,
    val fallbackReady: Boolean,
    val recoveryWarmupSeconds: Int,
    val recoveryReadyDurationSeconds: Int,
    val recoveryReady: Boolean,
    val fallbackCooldownSeconds: Int,
    val recoveryCooldownSeconds: Int,
    val receiverTelemetryAgeSeconds: Int,
    val senderTelemetryAgeSeconds: Int,
    val natStatus: String,
    val hostNatProbeAgeSeconds: Int,
    val clientNatProbeAgeSeconds: Int,
    val natProbeFresh: Boolean,
)

internal data class ControlPlaneDesiredStreamRequest(
    val width: Int?,
    val height: Int?,
    val fps: Int?,
    val bitrateBps: Int?,
    val captureCursor: Boolean?,
    val adaptiveMode: Boolean?,
    val preferredCodecs: List<String> = emptyList(),
    val presetId: String? = null,
)

internal data class ControlPlaneClientCapabilities(
    val supportedDecodeCodecs: List<String> = emptyList(),
    val lanAddresses: List<String> = emptyList(),
)

internal data class ControlPlaneAuthState(
    val mode: String,
    val label: String,
    val userAuthenticated: Boolean,
)

internal data class ControlPlaneManagedSessionState(
    val baseUrl: String,
    val sessionId: String,
    val sessionToken: String,
    val hostId: String,
    val hostDisplayName: String,
    val routeKind: String,
    val routeState: String,
    val routeVersion: Int = 0,
    val sessionHealth: String,
    val sessionHealthReason: String,
    val routeActionHint: String,
    val routeActionReason: String,
    val routeFallbackReadyDurationSeconds: Int = 0,
    val routeRecoveryReadyDurationSeconds: Int = 0,
    val recommendedSyncDelaySeconds: Int = 10,
    val transportLossLevel: String,
    val receiverTelemetryAgeSeconds: Int,
    val senderTelemetryAgeSeconds: Int,
    val lastRouteActionKind: String? = null,
    val lastRouteActionReason: String? = null,
    val lastRouteActionActor: String? = null,
    val lastRouteActionUtc: String? = null,
    val routeRecoveryCount: Int = 0,
    val routeRecoveryCooldownSeconds: Int = 0,
    val routeFallbackCount: Int = 0,
    val routeFallbackCooldownSeconds: Int = 0,
    val natStatus: String,
    val hostNatProbeAgeSeconds: Int = -1,
    val clientNatProbeAgeSeconds: Int = -1,
    val natProbeFresh: Boolean = false,
    val relayHost: String? = null,
    val relayPort: Int? = null,
    val receiverHost: String? = null,
    val receiverPort: Int? = null,
    val probeHost: String? = null,
    val probePort: Int? = null,
    val probeToken: String,
    val transportAnomalyKind: String = "unknown",
    val transportAnomalyReason: String = "cached state",
    val transportAnomalyConfidence: String = "low",
)

private data class NatProbeEcho(
    val observedAddress: String,
    val observedPort: Int,
    val localAddress: String?,
    val localPort: Int?,
)

internal class ControlPlaneClient {
    private companion object {
        private const val PREFS_NAME = "everty_control_plane"
        private const val PREF_BASE_URL = "base_url"
        private const val PREF_DEVICE_ID = "device_id"
        private const val PREF_DEVICE_SECRET = "device_secret"
        private const val PREF_REFRESH_TOKEN = "refresh_token"
        private const val PREF_REFRESH_EXPIRES_AT_MS = "refresh_expires_at_ms"
        private const val PREF_USER_EMAIL = "user_email"
        private const val PREF_USER_REFRESH_TOKEN = "user_refresh_token"
        private const val PREF_USER_REFRESH_EXPIRES_AT_MS = "user_refresh_expires_at_ms"
        private const val PREF_MANAGED_SESSION_BASE_URL = "managed_session_base_url"
        private const val PREF_MANAGED_SESSION_ID = "managed_session_id"
        private const val PREF_MANAGED_SESSION_TOKEN = "managed_session_token"
        private const val PREF_MANAGED_SESSION_HOST_ID = "managed_session_host_id"
        private const val PREF_MANAGED_SESSION_HOST_DISPLAY = "managed_session_host_display"
        private const val PREF_MANAGED_SESSION_ROUTE_KIND = "managed_session_route_kind"
        private const val PREF_MANAGED_SESSION_ROUTE_STATE = "managed_session_route_state"
        private const val PREF_MANAGED_SESSION_ROUTE_VERSION = "managed_session_route_version"
        private const val PREF_MANAGED_SESSION_HEALTH = "managed_session_health"
        private const val PREF_MANAGED_SESSION_HEALTH_REASON = "managed_session_health_reason"
        private const val PREF_MANAGED_SESSION_ROUTE_ACTION_HINT = "managed_session_route_action_hint"
        private const val PREF_MANAGED_SESSION_ROUTE_ACTION_REASON = "managed_session_route_action_reason"
        private const val PREF_MANAGED_SESSION_ROUTE_FALLBACK_READY = "managed_session_route_fallback_ready"
        private const val PREF_MANAGED_SESSION_ROUTE_RECOVERY_READY = "managed_session_route_recovery_ready"
        private const val PREF_MANAGED_SESSION_RECOMMENDED_SYNC_DELAY = "managed_session_recommended_sync_delay"
        private const val PREF_MANAGED_SESSION_TRANSPORT_LOSS = "managed_session_transport_loss"
        private const val PREF_MANAGED_SESSION_TRANSPORT_ANOMALY_KIND = "managed_session_transport_anomaly_kind"
        private const val PREF_MANAGED_SESSION_TRANSPORT_ANOMALY_REASON = "managed_session_transport_anomaly_reason"
        private const val PREF_MANAGED_SESSION_TRANSPORT_ANOMALY_CONFIDENCE = "managed_session_transport_anomaly_confidence"
        private const val PREF_MANAGED_SESSION_RECEIVER_AGE = "managed_session_receiver_age"
        private const val PREF_MANAGED_SESSION_SENDER_AGE = "managed_session_sender_age"
        private const val PREF_MANAGED_SESSION_LAST_ROUTE_ACTION_KIND = "managed_session_last_route_action_kind"
        private const val PREF_MANAGED_SESSION_LAST_ROUTE_ACTION_REASON = "managed_session_last_route_action_reason"
        private const val PREF_MANAGED_SESSION_LAST_ROUTE_ACTION_ACTOR = "managed_session_last_route_action_actor"
        private const val PREF_MANAGED_SESSION_LAST_ROUTE_ACTION_UTC = "managed_session_last_route_action_utc"
        private const val PREF_MANAGED_SESSION_ROUTE_RECOVERY_COUNT = "managed_session_route_recovery_count"
        private const val PREF_MANAGED_SESSION_ROUTE_RECOVERY_COOLDOWN = "managed_session_route_recovery_cooldown"
        private const val PREF_MANAGED_SESSION_ROUTE_FALLBACK_COUNT = "managed_session_route_fallback_count"
        private const val PREF_MANAGED_SESSION_ROUTE_FALLBACK_COOLDOWN = "managed_session_route_fallback_cooldown"
        private const val PREF_MANAGED_SESSION_NAT_STATUS = "managed_session_nat_status"
        private const val PREF_MANAGED_SESSION_HOST_NAT_PROBE_AGE = "managed_session_host_nat_probe_age"
        private const val PREF_MANAGED_SESSION_CLIENT_NAT_PROBE_AGE = "managed_session_client_nat_probe_age"
        private const val PREF_MANAGED_SESSION_NAT_PROBE_FRESH = "managed_session_nat_probe_fresh"
        private const val PREF_MANAGED_SESSION_RELAY_HOST = "managed_session_relay_host"
        private const val PREF_MANAGED_SESSION_RELAY_PORT = "managed_session_relay_port"
        private const val PREF_MANAGED_SESSION_RECEIVER_HOST = "managed_session_receiver_host"
        private const val PREF_MANAGED_SESSION_RECEIVER_PORT = "managed_session_receiver_port"
        private const val PREF_MANAGED_SESSION_PROBE_HOST = "managed_session_probe_host"
        private const val PREF_MANAGED_SESSION_PROBE_PORT = "managed_session_probe_port"
        private const val PREF_MANAGED_SESSION_PROBE_TOKEN = "managed_session_probe_token"
    }

    private val appContext: Context
    private val prefs by lazy { appContext.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE) }
    @Volatile private var cachedAccessToken: String? = null
    @Volatile private var cachedAccessExpiresAtMs: Long = 0L
    @Volatile private var cachedAccessBaseUrl: String? = null

    constructor(context: Context) {
        appContext = context.applicationContext
    }

    fun getAuthState(): ControlPlaneAuthState {
        val userEmail = prefs.getString(PREF_USER_EMAIL, null)
        if (!userEmail.isNullOrBlank()) {
            return ControlPlaneAuthState("user", userEmail, userAuthenticated = true)
        }

        val deviceId = prefs.getString(PREF_DEVICE_ID, null)
        if (!deviceId.isNullOrBlank()) {
            return ControlPlaneAuthState("device", deviceId, userAuthenticated = false)
        }

        return ControlPlaneAuthState("anonymous", "-", userAuthenticated = false)
    }

    fun getManagedSessionState(baseUrl: String): ControlPlaneManagedSessionState? {
        val normalizedBaseUrl = baseUrl.trim().trimEnd('/')
        val storedBaseUrl = prefs.getString(PREF_MANAGED_SESSION_BASE_URL, null).orEmpty()
        if (normalizedBaseUrl.isBlank() || !storedBaseUrl.equals(normalizedBaseUrl, ignoreCase = true)) {
            return null
        }
        val sessionId = prefs.getString(PREF_MANAGED_SESSION_ID, null).orEmpty()
        val sessionToken = prefs.getString(PREF_MANAGED_SESSION_TOKEN, null).orEmpty()
        val hostId = prefs.getString(PREF_MANAGED_SESSION_HOST_ID, null).orEmpty()
        val hostDisplay = prefs.getString(PREF_MANAGED_SESSION_HOST_DISPLAY, null).orEmpty()
        val routeKind = prefs.getString(PREF_MANAGED_SESSION_ROUTE_KIND, null).orEmpty()
        val routeState = prefs.getString(PREF_MANAGED_SESSION_ROUTE_STATE, null).orEmpty()
        val routeVersion = prefs.getInt(PREF_MANAGED_SESSION_ROUTE_VERSION, 0)
        val sessionHealth = prefs.getString(PREF_MANAGED_SESSION_HEALTH, null).orEmpty()
        val sessionHealthReason = prefs.getString(PREF_MANAGED_SESSION_HEALTH_REASON, null).orEmpty()
        val routeActionHint = prefs.getString(PREF_MANAGED_SESSION_ROUTE_ACTION_HINT, null).orEmpty()
        val routeActionReason = prefs.getString(PREF_MANAGED_SESSION_ROUTE_ACTION_REASON, null).orEmpty()
        val routeFallbackReadyDurationSeconds = prefs.getInt(PREF_MANAGED_SESSION_ROUTE_FALLBACK_READY, 0)
        val routeRecoveryReadyDurationSeconds = prefs.getInt(PREF_MANAGED_SESSION_ROUTE_RECOVERY_READY, 0)
        val recommendedSyncDelaySeconds = prefs.getInt(PREF_MANAGED_SESSION_RECOMMENDED_SYNC_DELAY, 10)
        val transportLossLevel = prefs.getString(PREF_MANAGED_SESSION_TRANSPORT_LOSS, null).orEmpty()
        val transportAnomalyKind = prefs.getString(PREF_MANAGED_SESSION_TRANSPORT_ANOMALY_KIND, null).orEmpty()
        val transportAnomalyReason = prefs.getString(PREF_MANAGED_SESSION_TRANSPORT_ANOMALY_REASON, null).orEmpty()
        val transportAnomalyConfidence = prefs.getString(PREF_MANAGED_SESSION_TRANSPORT_ANOMALY_CONFIDENCE, null).orEmpty()
        val receiverTelemetryAgeSeconds = prefs.getInt(PREF_MANAGED_SESSION_RECEIVER_AGE, -1)
        val senderTelemetryAgeSeconds = prefs.getInt(PREF_MANAGED_SESSION_SENDER_AGE, -1)
        val lastRouteActionKind = prefs.getString(PREF_MANAGED_SESSION_LAST_ROUTE_ACTION_KIND, null)
        val lastRouteActionReason = prefs.getString(PREF_MANAGED_SESSION_LAST_ROUTE_ACTION_REASON, null)
        val lastRouteActionActor = prefs.getString(PREF_MANAGED_SESSION_LAST_ROUTE_ACTION_ACTOR, null)
        val lastRouteActionUtc = prefs.getString(PREF_MANAGED_SESSION_LAST_ROUTE_ACTION_UTC, null)
        val routeRecoveryCount = prefs.getInt(PREF_MANAGED_SESSION_ROUTE_RECOVERY_COUNT, 0)
        val routeRecoveryCooldownSeconds = prefs.getInt(PREF_MANAGED_SESSION_ROUTE_RECOVERY_COOLDOWN, 0)
        val routeFallbackCount = prefs.getInt(PREF_MANAGED_SESSION_ROUTE_FALLBACK_COUNT, 0)
        val routeFallbackCooldownSeconds = prefs.getInt(PREF_MANAGED_SESSION_ROUTE_FALLBACK_COOLDOWN, 0)
        val natStatus = prefs.getString(PREF_MANAGED_SESSION_NAT_STATUS, null).orEmpty()
        val hostNatProbeAgeSeconds = prefs.getInt(PREF_MANAGED_SESSION_HOST_NAT_PROBE_AGE, -1)
        val clientNatProbeAgeSeconds = prefs.getInt(PREF_MANAGED_SESSION_CLIENT_NAT_PROBE_AGE, -1)
        val natProbeFresh = prefs.getBoolean(PREF_MANAGED_SESSION_NAT_PROBE_FRESH, false)
        val probeToken = prefs.getString(PREF_MANAGED_SESSION_PROBE_TOKEN, null).orEmpty()
        if (sessionId.isBlank() || sessionToken.isBlank() || hostId.isBlank() || hostDisplay.isBlank() || routeKind.isBlank() || natStatus.isBlank() || probeToken.isBlank()) {
            return null
        }
        return ControlPlaneManagedSessionState(
            baseUrl = normalizedBaseUrl,
            sessionId = sessionId,
            sessionToken = sessionToken,
            hostId = hostId,
            hostDisplayName = hostDisplay,
            routeKind = routeKind,
            routeState = routeState.ifBlank { routeKind },
            routeVersion = routeVersion.coerceAtLeast(0),
            sessionHealth = sessionHealth.ifBlank { "syncing" },
            sessionHealthReason = sessionHealthReason.ifBlank { "cached state" },
            routeActionHint = routeActionHint.ifBlank { "wait_for_telemetry" },
            routeActionReason = routeActionReason.ifBlank { "cached state" },
            routeFallbackReadyDurationSeconds = routeFallbackReadyDurationSeconds.coerceAtLeast(0),
            routeRecoveryReadyDurationSeconds = routeRecoveryReadyDurationSeconds.coerceAtLeast(0),
            recommendedSyncDelaySeconds = recommendedSyncDelaySeconds.coerceIn(5, 60),
            transportLossLevel = transportLossLevel.ifBlank { "unknown" },
            transportAnomalyKind = transportAnomalyKind.ifBlank { "unknown" },
            transportAnomalyReason = transportAnomalyReason.ifBlank { "cached state" },
            transportAnomalyConfidence = transportAnomalyConfidence.ifBlank { "low" },
            receiverTelemetryAgeSeconds = receiverTelemetryAgeSeconds,
            senderTelemetryAgeSeconds = senderTelemetryAgeSeconds,
            lastRouteActionKind = lastRouteActionKind,
            lastRouteActionReason = lastRouteActionReason,
            lastRouteActionActor = lastRouteActionActor,
            lastRouteActionUtc = lastRouteActionUtc,
            routeRecoveryCount = routeRecoveryCount.coerceAtLeast(0),
            routeRecoveryCooldownSeconds = routeRecoveryCooldownSeconds.coerceAtLeast(0),
            routeFallbackCount = routeFallbackCount.coerceAtLeast(0),
            routeFallbackCooldownSeconds = routeFallbackCooldownSeconds.coerceAtLeast(0),
            natStatus = natStatus,
            hostNatProbeAgeSeconds = hostNatProbeAgeSeconds,
            clientNatProbeAgeSeconds = clientNatProbeAgeSeconds,
            natProbeFresh = natProbeFresh,
            relayHost = prefs.getString(PREF_MANAGED_SESSION_RELAY_HOST, null)?.takeIf { it.isNotBlank() },
            relayPort = prefs.getInt(PREF_MANAGED_SESSION_RELAY_PORT, 0).takeIf { it > 0 },
            receiverHost = prefs.getString(PREF_MANAGED_SESSION_RECEIVER_HOST, null)?.takeIf { it.isNotBlank() },
            receiverPort = prefs.getInt(PREF_MANAGED_SESSION_RECEIVER_PORT, 0).takeIf { it > 0 },
            probeHost = prefs.getString(PREF_MANAGED_SESSION_PROBE_HOST, null)?.takeIf { it.isNotBlank() },
            probePort = prefs.getInt(PREF_MANAGED_SESSION_PROBE_PORT, 0).takeIf { it > 0 },
            probeToken = probeToken,
        )
    }

    fun saveManagedSessionState(
        baseUrl: String,
        sessionId: String,
        sessionToken: String,
        hostId: String,
        hostDisplayName: String,
        routeKind: String,
        routeState: String,
        routeVersion: Int = 0,
        sessionHealth: String,
        sessionHealthReason: String,
        routeActionHint: String,
        routeActionReason: String,
        routeFallbackReadyDurationSeconds: Int = 0,
        routeRecoveryReadyDurationSeconds: Int = 0,
        recommendedSyncDelaySeconds: Int = 10,
        transportLossLevel: String,
        transportAnomalyKind: String = "unknown",
        transportAnomalyReason: String = "unspecified",
        transportAnomalyConfidence: String = "low",
        receiverTelemetryAgeSeconds: Int,
        senderTelemetryAgeSeconds: Int,
        lastRouteActionKind: String? = null,
        lastRouteActionReason: String? = null,
        lastRouteActionActor: String? = null,
        lastRouteActionUtc: String? = null,
        routeRecoveryCount: Int = 0,
        routeRecoveryCooldownSeconds: Int = 0,
        routeFallbackCount: Int = 0,
        routeFallbackCooldownSeconds: Int = 0,
        natStatus: String,
        hostNatProbeAgeSeconds: Int = -1,
        clientNatProbeAgeSeconds: Int = -1,
        natProbeFresh: Boolean = false,
        relayHost: String?,
        relayPort: Int?,
        receiverHost: String?,
        receiverPort: Int?,
        probeHost: String?,
        probePort: Int?,
        probeToken: String,
    ) {
        val normalizedBaseUrl = baseUrl.trim().trimEnd('/')
        prefs.edit()
            .putString(PREF_MANAGED_SESSION_BASE_URL, normalizedBaseUrl)
            .putString(PREF_MANAGED_SESSION_ID, sessionId)
            .putString(PREF_MANAGED_SESSION_TOKEN, sessionToken)
            .putString(PREF_MANAGED_SESSION_HOST_ID, hostId)
            .putString(PREF_MANAGED_SESSION_HOST_DISPLAY, hostDisplayName)
            .putString(PREF_MANAGED_SESSION_ROUTE_KIND, routeKind)
            .putString(PREF_MANAGED_SESSION_ROUTE_STATE, routeState)
            .putInt(PREF_MANAGED_SESSION_ROUTE_VERSION, routeVersion)
            .putString(PREF_MANAGED_SESSION_HEALTH, sessionHealth)
            .putString(PREF_MANAGED_SESSION_HEALTH_REASON, sessionHealthReason)
            .putString(PREF_MANAGED_SESSION_ROUTE_ACTION_HINT, routeActionHint)
            .putString(PREF_MANAGED_SESSION_ROUTE_ACTION_REASON, routeActionReason)
            .putInt(PREF_MANAGED_SESSION_ROUTE_FALLBACK_READY, routeFallbackReadyDurationSeconds.coerceAtLeast(0))
            .putInt(PREF_MANAGED_SESSION_ROUTE_RECOVERY_READY, routeRecoveryReadyDurationSeconds.coerceAtLeast(0))
            .putInt(PREF_MANAGED_SESSION_RECOMMENDED_SYNC_DELAY, recommendedSyncDelaySeconds.coerceIn(5, 60))
            .putString(PREF_MANAGED_SESSION_TRANSPORT_LOSS, transportLossLevel)
            .putString(PREF_MANAGED_SESSION_TRANSPORT_ANOMALY_KIND, transportAnomalyKind)
            .putString(PREF_MANAGED_SESSION_TRANSPORT_ANOMALY_REASON, transportAnomalyReason)
            .putString(PREF_MANAGED_SESSION_TRANSPORT_ANOMALY_CONFIDENCE, transportAnomalyConfidence)
            .putInt(PREF_MANAGED_SESSION_RECEIVER_AGE, receiverTelemetryAgeSeconds)
            .putInt(PREF_MANAGED_SESSION_SENDER_AGE, senderTelemetryAgeSeconds)
            .putString(PREF_MANAGED_SESSION_LAST_ROUTE_ACTION_KIND, lastRouteActionKind)
            .putString(PREF_MANAGED_SESSION_LAST_ROUTE_ACTION_REASON, lastRouteActionReason)
            .putString(PREF_MANAGED_SESSION_LAST_ROUTE_ACTION_ACTOR, lastRouteActionActor)
            .putString(PREF_MANAGED_SESSION_LAST_ROUTE_ACTION_UTC, lastRouteActionUtc)
            .putInt(PREF_MANAGED_SESSION_ROUTE_RECOVERY_COUNT, routeRecoveryCount.coerceAtLeast(0))
            .putInt(PREF_MANAGED_SESSION_ROUTE_RECOVERY_COOLDOWN, routeRecoveryCooldownSeconds.coerceAtLeast(0))
            .putInt(PREF_MANAGED_SESSION_ROUTE_FALLBACK_COUNT, routeFallbackCount)
            .putInt(PREF_MANAGED_SESSION_ROUTE_FALLBACK_COOLDOWN, routeFallbackCooldownSeconds)
            .putString(PREF_MANAGED_SESSION_NAT_STATUS, natStatus)
            .putInt(PREF_MANAGED_SESSION_HOST_NAT_PROBE_AGE, hostNatProbeAgeSeconds)
            .putInt(PREF_MANAGED_SESSION_CLIENT_NAT_PROBE_AGE, clientNatProbeAgeSeconds)
            .putBoolean(PREF_MANAGED_SESSION_NAT_PROBE_FRESH, natProbeFresh)
            .putString(PREF_MANAGED_SESSION_RELAY_HOST, relayHost)
            .putInt(PREF_MANAGED_SESSION_RELAY_PORT, relayPort ?: 0)
            .putString(PREF_MANAGED_SESSION_RECEIVER_HOST, receiverHost)
            .putInt(PREF_MANAGED_SESSION_RECEIVER_PORT, receiverPort ?: 0)
            .putString(PREF_MANAGED_SESSION_PROBE_HOST, probeHost)
            .putInt(PREF_MANAGED_SESSION_PROBE_PORT, probePort ?: 0)
            .putString(PREF_MANAGED_SESSION_PROBE_TOKEN, probeToken)
            .apply()
    }

    fun clearManagedSessionState(baseUrl: String) {
        val normalizedBaseUrl = baseUrl.trim().trimEnd('/')
        val storedBaseUrl = prefs.getString(PREF_MANAGED_SESSION_BASE_URL, null).orEmpty()
        if (normalizedBaseUrl.isBlank() || !storedBaseUrl.equals(normalizedBaseUrl, ignoreCase = true)) {
            return
        }
        clearAllManagedSessionState()
    }

    fun clearAllManagedSessionState() {
        prefs.edit()
            .remove(PREF_MANAGED_SESSION_BASE_URL)
            .remove(PREF_MANAGED_SESSION_ID)
            .remove(PREF_MANAGED_SESSION_TOKEN)
            .remove(PREF_MANAGED_SESSION_HOST_ID)
            .remove(PREF_MANAGED_SESSION_HOST_DISPLAY)
            .remove(PREF_MANAGED_SESSION_ROUTE_KIND)
            .remove(PREF_MANAGED_SESSION_ROUTE_STATE)
            .remove(PREF_MANAGED_SESSION_ROUTE_VERSION)
            .remove(PREF_MANAGED_SESSION_HEALTH)
            .remove(PREF_MANAGED_SESSION_HEALTH_REASON)
            .remove(PREF_MANAGED_SESSION_ROUTE_ACTION_HINT)
            .remove(PREF_MANAGED_SESSION_ROUTE_ACTION_REASON)
            .remove(PREF_MANAGED_SESSION_ROUTE_FALLBACK_READY)
            .remove(PREF_MANAGED_SESSION_ROUTE_RECOVERY_READY)
            .remove(PREF_MANAGED_SESSION_RECOMMENDED_SYNC_DELAY)
            .remove(PREF_MANAGED_SESSION_TRANSPORT_LOSS)
            .remove(PREF_MANAGED_SESSION_TRANSPORT_ANOMALY_KIND)
            .remove(PREF_MANAGED_SESSION_TRANSPORT_ANOMALY_REASON)
            .remove(PREF_MANAGED_SESSION_TRANSPORT_ANOMALY_CONFIDENCE)
            .remove(PREF_MANAGED_SESSION_RECEIVER_AGE)
            .remove(PREF_MANAGED_SESSION_SENDER_AGE)
            .remove(PREF_MANAGED_SESSION_LAST_ROUTE_ACTION_KIND)
            .remove(PREF_MANAGED_SESSION_LAST_ROUTE_ACTION_REASON)
            .remove(PREF_MANAGED_SESSION_LAST_ROUTE_ACTION_ACTOR)
            .remove(PREF_MANAGED_SESSION_LAST_ROUTE_ACTION_UTC)
            .remove(PREF_MANAGED_SESSION_ROUTE_RECOVERY_COUNT)
            .remove(PREF_MANAGED_SESSION_ROUTE_RECOVERY_COOLDOWN)
            .remove(PREF_MANAGED_SESSION_ROUTE_FALLBACK_COUNT)
            .remove(PREF_MANAGED_SESSION_ROUTE_FALLBACK_COOLDOWN)
            .remove(PREF_MANAGED_SESSION_NAT_STATUS)
            .remove(PREF_MANAGED_SESSION_HOST_NAT_PROBE_AGE)
            .remove(PREF_MANAGED_SESSION_CLIENT_NAT_PROBE_AGE)
            .remove(PREF_MANAGED_SESSION_NAT_PROBE_FRESH)
            .remove(PREF_MANAGED_SESSION_RELAY_HOST)
            .remove(PREF_MANAGED_SESSION_RELAY_PORT)
            .remove(PREF_MANAGED_SESSION_RECEIVER_HOST)
            .remove(PREF_MANAGED_SESSION_RECEIVER_PORT)
            .remove(PREF_MANAGED_SESSION_PROBE_HOST)
            .remove(PREF_MANAGED_SESSION_PROBE_PORT)
            .remove(PREF_MANAGED_SESSION_PROBE_TOKEN)
            .apply()
    }

    suspend fun registerUser(baseUrl: String, email: String, password: String): ControlPlaneAuthState = withContext(Dispatchers.IO) {
        authenticateUser(baseUrl, "/api/auth/users/register", email, password)
    }

    suspend fun loginUser(baseUrl: String, email: String, password: String): ControlPlaneAuthState = withContext(Dispatchers.IO) {
        authenticateUser(baseUrl, "/api/auth/users/login", email, password)
    }

    suspend fun listHosts(baseUrl: String): List<ControlPlaneHostSummary> = withContext(Dispatchers.IO) {
        val accessToken = ensureAccessToken(baseUrl)
        val response = executeJsonRequest(
            method = "GET",
            baseUrl = baseUrl,
            path = "/api/hosts",
            requestBody = null,
            accessToken = accessToken,
        )

        val array = when {
            response.trimStart().startsWith("[") -> JSONArray(response)
            else -> JSONObject(response).optJSONArray("value") ?: JSONArray()
        }

        buildList(array.length()) {
            for (index in 0 until array.length()) {
                val item = array.getJSONObject(index)
                add(
                        ControlPlaneHostSummary(
                            hostId = item.optString("hostId"),
                            hostCode = item.optString("hostCode"),
                            displayName = item.optString("displayName"),
                            region = item.optString("region"),
                        online = item.optBoolean("online"),
                        availability = item.opt("availability")?.toString().orEmpty(),
                        activeSessionId = item.optString("activeSessionId").takeIf { it.isNotBlank() },
                        supportsHevc = item.optBoolean("supportsHevc"),
                        supportsAudio = item.optBoolean("supportsAudio"),
                        supportsGamepad = item.optBoolean("supportsGamepad"),
                        pricePerHour = item.optDouble("pricePerHour").takeIf { item.has("pricePerHour") },
                        currency = item.optString("currency").takeIf { it.isNotBlank() },
                        description = item.optString("description").takeIf { it.isNotBlank() },
                    ),
                )
            }
        }
    }

    suspend fun createSession(
        baseUrl: String,
        hostId: String,
        clientLabel: String,
        clientRegion: String,
        codecPreference: String?,
        preferRelay: Boolean,
        audioRequested: Boolean,
        controllerCount: Int,
        leaseMinutes: Int,
        receiverAddress: String,
        receiverPort: Int,
        desiredStream: ControlPlaneDesiredStreamRequest,
        clientCapabilities: ControlPlaneClientCapabilities? = null,
    ): ControlPlaneSessionLease = withContext(Dispatchers.IO) {
        val accessToken = ensureAccessToken(baseUrl)
        val request = JSONObject().apply {
            put("hostId", hostId)
            put("clientLabel", clientLabel)
            put("clientRegion", clientRegion)
            put("codecPreference", codecPreference)
            if (desiredStream.preferredCodecs.isNotEmpty()) {
                put("preferredCodecs", JSONArray(desiredStream.preferredCodecs))
            }
            if (!desiredStream.presetId.isNullOrBlank()) {
                put("presetId", desiredStream.presetId)
            }
            put("preferRelay", preferRelay)
            put("replaceExistingActorSession", true)
            put("audioRequested", audioRequested)
            put("controllerCount", controllerCount)
            put("leaseMinutes", leaseMinutes)
            put("receiverAddress", receiverAddress)
            put("receiverPort", receiverPort)
            put("requestedWidth", desiredStream.width ?: 0)
            put("requestedHeight", desiredStream.height ?: 0)
            put("requestedFps", desiredStream.fps ?: 0)
            put("requestedBitrateBps", desiredStream.bitrateBps ?: 0)
            if (desiredStream.captureCursor != null) {
                put("captureCursor", desiredStream.captureCursor)
            }
            if (desiredStream.adaptiveMode != null) {
                put("adaptiveMode", desiredStream.adaptiveMode)
            }
            if (clientCapabilities != null) {
                put("capabilities", JSONObject().apply {
                    if (clientCapabilities.supportedDecodeCodecs.isNotEmpty()) {
                        put("supportedDecodeCodecs", JSONArray(clientCapabilities.supportedDecodeCodecs))
                    }
                    if (clientCapabilities.lanAddresses.isNotEmpty()) {
                        put("lanAddresses", JSONArray(clientCapabilities.lanAddresses))
                    }
                })
            }
        }

        val response = executeJsonRequest(
            method = "POST",
            baseUrl = baseUrl,
            path = "/api/sessions",
            requestBody = request.toString(),
            accessToken = accessToken,
        )
        parseLease(JSONObject(response))
    }

    suspend fun stopSession(
        baseUrl: String,
        sessionId: String,
        sessionToken: String,
        reason: String,
    ) = withContext(Dispatchers.IO) {
        Log.d("EVRT", "STOP API BEGIN | baseUrl=${baseUrl.trim()} | sessionId=$sessionId | reason=$reason")
        val accessToken = ensureAccessToken(baseUrl)
        val request = JSONObject().apply {
            put("sessionToken", sessionToken)
            put("reason", reason)
        }
        executeJsonRequest(
            method = "POST",
            baseUrl = baseUrl,
            path = "/api/sessions/$sessionId/stop",
            requestBody = request.toString(),
            accessToken = accessToken,
        )
        Log.d("EVRT", "STOP API OK | sessionId=$sessionId")
    }

    suspend fun activateSession(
        baseUrl: String,
        sessionId: String,
        sessionToken: String,
    ) = withContext(Dispatchers.IO) {
        val accessToken = ensureAccessToken(baseUrl)
        val request = JSONObject().apply {
            put("sessionToken", sessionToken)
            put("reason", "receiver_ready")
        }
        executeJsonRequest(
            method = "POST",
            baseUrl = baseUrl,
            path = "/api/sessions/$sessionId/activate",
            requestBody = request.toString(),
            accessToken = accessToken,
        )
    }

    suspend fun keepAliveSession(
        baseUrl: String,
        sessionId: String,
        sessionToken: String,
    ) = withContext(Dispatchers.IO) {
        val accessToken = ensureAccessToken(baseUrl)
        val request = JSONObject().apply {
            put("sessionToken", sessionToken)
            put("reason", "managed_sync")
        }
        executeJsonRequest(
            method = "POST",
            baseUrl = baseUrl,
            path = "/api/sessions/$sessionId/keepalive",
            requestBody = request.toString(),
            accessToken = accessToken,
        )
    }

    suspend fun fallbackManagedSessionRoute(
        baseUrl: String,
        sessionId: String,
        sessionToken: String,
        reason: String = "managed_sync_failure",
    ): ControlPlaneConnectInstructions = withContext(Dispatchers.IO) {
        val accessToken = ensureAccessToken(baseUrl)
        val request = JSONObject().apply {
            put("sessionToken", sessionToken)
            put("reason", reason)
        }
        val response = executeJsonRequest(
            method = "POST",
            baseUrl = baseUrl,
            path = "/api/sessions/$sessionId/route/fallback",
            requestBody = request.toString(),
            accessToken = accessToken,
        )
        parseConnectInstructions(JSONObject(response))
    }

    suspend fun recoverManagedSessionRoute(
        baseUrl: String,
        sessionId: String,
        sessionToken: String,
        reason: String = "managed_route_recovery",
    ): ControlPlaneConnectInstructions = withContext(Dispatchers.IO) {
        val accessToken = ensureAccessToken(baseUrl)
        val request = JSONObject().apply {
            put("sessionToken", sessionToken)
            put("reason", reason)
        }
        val response = executeJsonRequest(
            method = "POST",
            baseUrl = baseUrl,
            path = "/api/sessions/$sessionId/route/recover",
            requestBody = request.toString(),
            accessToken = accessToken,
        )
        parseConnectInstructions(JSONObject(response))
    }

    suspend fun getConnectInstructions(
        baseUrl: String,
        sessionId: String,
        sessionToken: String,
    ): ControlPlaneConnectInstructions = withContext(Dispatchers.IO) {
        val accessToken = ensureAccessToken(baseUrl)
        val response = executeJsonRequest(
            method = "GET",
            baseUrl = baseUrl,
            path = "/api/sessions/$sessionId/connect?sessionToken=$sessionToken",
            requestBody = null,
            accessToken = accessToken,
        )
        parseConnectInstructions(JSONObject(response))
    }

    suspend fun getRoutePolicy(
        baseUrl: String,
        sessionId: String,
        sessionToken: String,
    ): ControlPlaneRoutePolicy = withContext(Dispatchers.IO) {
        val accessToken = ensureAccessToken(baseUrl)
        val response = executeJsonRequest(
            method = "GET",
            baseUrl = baseUrl,
            path = "/api/sessions/$sessionId/route/policy?sessionToken=$sessionToken",
            requestBody = null,
            accessToken = accessToken,
        )
        parseRoutePolicy(JSONObject(response))
    }

    suspend fun publishNatProbe(
        baseUrl: String,
        sessionId: String,
        sessionToken: String,
        probeToken: String,
        probeHost: String,
        probePort: Int,
        role: String,
    ) = withContext(Dispatchers.IO) {
        if (probeToken.isBlank() || probeHost.isBlank() || probePort !in 1..65535) {
            return@withContext
        }

        val observed = runNatProbe(sessionId, probeToken, probeHost, probePort, role) ?: return@withContext
        val accessToken = ensureAccessToken(baseUrl)
        val request = JSONObject().apply {
            put("sessionToken", sessionToken)
            put("probeToken", probeToken)
            put("role", role)
            put("observedAddress", observed.observedAddress)
            put("observedPort", observed.observedPort)
            if (observed.localAddress != null) {
                put("localAddress", observed.localAddress)
            }
            if (observed.localPort != null) {
                put("localPort", observed.localPort)
            }
            put("networkType", "udp")
        }
        executeJsonRequest(
            method = "POST",
            baseUrl = baseUrl,
            path = "/api/sessions/$sessionId/nat/probe",
            requestBody = request.toString(),
            accessToken = accessToken,
        )
    }

    suspend fun publishReceiverFeedback(
        baseUrl: String,
        sessionId: String,
        sessionToken: String,
        pressure: String,
        decodeFps: Int,
        queueDrops: Long,
        queueDropBurst: Long,
        decodeDeltaMs: Int,
        presentDeltaMs: Int,
        pulseEstimateMs: Int,
        inputEstimateMs: Int,
    ) = withContext(Dispatchers.IO) {
        val accessToken = ensureAccessToken(baseUrl)
        val payload = JSONObject().apply {
            put("pressure", pressure)
            put("decodeFps", decodeFps)
            put("queueDrops", queueDrops)
            put("queueDropBurst", queueDropBurst)
            put("decodeDeltaMs", decodeDeltaMs)
            put("presentDeltaMs", presentDeltaMs)
            put("pulseEstimateMs", pulseEstimateMs)
            put("inputEstimateMs", inputEstimateMs)
        }
        val request = JSONObject().apply {
            put("sessionId", sessionId)
            put("sessionToken", sessionToken)
            put("source", "android_pc_receiver")
            put("eventType", "receiver_feedback")
            put("payload", payload)
        }
        executeJsonRequest(
            method = "POST",
            baseUrl = baseUrl,
            path = "/api/telemetry/session",
            requestBody = request.toString(),
            accessToken = accessToken,
        )
    }

    private fun ensureAccessToken(baseUrl: String): String {
        val normalizedBaseUrl = baseUrl.trim().trimEnd('/')
        val nowMs = System.currentTimeMillis()
        val inMemoryToken = cachedAccessToken
        if (!inMemoryToken.isNullOrBlank() &&
            cachedAccessBaseUrl == normalizedBaseUrl &&
            cachedAccessExpiresAtMs > nowMs + 60_000L
        ) {
            return inMemoryToken
        }

        val persistedBaseUrl = prefs.getString(PREF_BASE_URL, null)
        val userRefreshToken = prefs.getString(PREF_USER_REFRESH_TOKEN, null)
        val userRefreshExpiresAtMs = prefs.getLong(PREF_USER_REFRESH_EXPIRES_AT_MS, 0L)
        if (persistedBaseUrl == normalizedBaseUrl &&
            !userRefreshToken.isNullOrBlank() &&
            userRefreshExpiresAtMs > nowMs + 60_000L
        ) {
            runCatching {
                val refreshRequest = JSONObject().apply {
                    put("refreshToken", userRefreshToken)
                }
                val refreshResponse = executeJsonRequest(
                    method = "POST",
                    baseUrl = normalizedBaseUrl,
                    path = "/api/auth/users/refresh",
                    requestBody = refreshRequest.toString(),
                    accessToken = null,
                )
                val refreshJson = JSONObject(refreshResponse)
                val rotatedAccessToken = refreshJson.optString("accessToken")
                val rotatedRefreshToken = refreshJson.optString("refreshToken")
                val rotatedUserEmail = refreshJson.optJSONObject("user")?.optString("email").orEmpty()
                val rotatedAccessExpiresAtMs = nowMs + 11L * 60L * 60L * 1000L
                val rotatedRefreshExpiresAtMs = nowMs + 29L * 24L * 60L * 60L * 1000L
                if (rotatedAccessToken.isBlank() || rotatedRefreshToken.isBlank()) {
                    error("Control plane user refresh returned incomplete credentials.")
                }

                cachedAccessToken = rotatedAccessToken
                cachedAccessExpiresAtMs = rotatedAccessExpiresAtMs
                cachedAccessBaseUrl = normalizedBaseUrl
                prefs.edit()
                    .putString(PREF_BASE_URL, normalizedBaseUrl)
                    .putString(PREF_USER_EMAIL, rotatedUserEmail)
                    .putString(PREF_USER_REFRESH_TOKEN, rotatedRefreshToken)
                    .putLong(PREF_USER_REFRESH_EXPIRES_AT_MS, rotatedRefreshExpiresAtMs)
                    .apply()
                return rotatedAccessToken
            }
        }

        val refreshToken = prefs.getString(PREF_REFRESH_TOKEN, null)
        val refreshExpiresAtMs = prefs.getLong(PREF_REFRESH_EXPIRES_AT_MS, 0L)
        if (persistedBaseUrl == normalizedBaseUrl &&
            !refreshToken.isNullOrBlank() &&
            refreshExpiresAtMs > nowMs + 60_000L
        ) {
            runCatching {
                val refreshRequest = JSONObject().apply {
                    put("refreshToken", refreshToken)
                }
                val refreshResponse = executeJsonRequest(
                    method = "POST",
                    baseUrl = normalizedBaseUrl,
                    path = "/api/auth/refresh",
                    requestBody = refreshRequest.toString(),
                    accessToken = null,
                )
                val refreshJson = JSONObject(refreshResponse)
                val rotatedAccessToken = refreshJson.optString("accessToken")
                val rotatedRefreshToken = refreshJson.optString("refreshToken")
                val rotatedAccessExpiresAtMs = nowMs + 11L * 60L * 60L * 1000L
                val rotatedRefreshExpiresAtMs = nowMs + 29L * 24L * 60L * 60L * 1000L
                if (rotatedAccessToken.isBlank() || rotatedRefreshToken.isBlank()) {
                    error("Control plane refresh returned incomplete credentials.")
                }

                cachedAccessToken = rotatedAccessToken
                cachedAccessExpiresAtMs = rotatedAccessExpiresAtMs
                cachedAccessBaseUrl = normalizedBaseUrl
                prefs.edit()
                    .putString(PREF_BASE_URL, normalizedBaseUrl)
                    .putString(PREF_REFRESH_TOKEN, rotatedRefreshToken)
                    .putLong(PREF_REFRESH_EXPIRES_AT_MS, rotatedRefreshExpiresAtMs)
                    .apply()
                return rotatedAccessToken
            }
        }

        val request = JSONObject().apply {
            val existingDeviceId = prefs.getString(PREF_DEVICE_ID, null)
            val existingDeviceSecret = prefs.getString(PREF_DEVICE_SECRET, null)
            if (!existingDeviceId.isNullOrBlank()) {
                put("deviceId", existingDeviceId)
            }
            if (!existingDeviceSecret.isNullOrBlank()) {
                put("deviceSecret", existingDeviceSecret)
            }
            put("deviceLabel", "${Build.MANUFACTURER} ${Build.MODEL}".trim())
            put("platform", "android")
        }

        val response = executeJsonRequest(
            method = "POST",
            baseUrl = baseUrl,
            path = "/api/auth/device-login",
            requestBody = request.toString(),
            accessToken = null,
        )
        val json = JSONObject(response)
        val deviceId = json.optString("deviceId")
        val deviceSecret = json.optString("deviceSecret")
        val accessToken = json.optString("accessToken")
        val refreshTokenResponse = json.optString("refreshToken")
        val accessExpiresAtMs = nowMs + 11L * 60L * 60L * 1000L
        val refreshExpiresAtMsResponse = nowMs + 29L * 24L * 60L * 60L * 1000L
        if (deviceId.isBlank() || deviceSecret.isBlank() || accessToken.isBlank() || refreshTokenResponse.isBlank()) {
            throw IllegalStateException("Control plane auth returned incomplete device credentials.")
        }
        cachedAccessToken = accessToken
        cachedAccessExpiresAtMs = accessExpiresAtMs
        cachedAccessBaseUrl = normalizedBaseUrl
        prefs.edit()
            .putString(PREF_BASE_URL, normalizedBaseUrl)
            .putString(PREF_DEVICE_ID, deviceId)
            .putString(PREF_DEVICE_SECRET, deviceSecret)
            .putString(PREF_REFRESH_TOKEN, refreshTokenResponse)
            .putLong(PREF_REFRESH_EXPIRES_AT_MS, refreshExpiresAtMsResponse)
            .apply()
        return accessToken
    }

    private fun authenticateUser(baseUrl: String, path: String, email: String, password: String): ControlPlaneAuthState {
        val normalizedBaseUrl = baseUrl.trim().trimEnd('/')
        require(normalizedBaseUrl.isNotBlank()) { "Control plane URL is required." }
        val request = JSONObject().apply {
            put("email", email.trim())
            put("password", password)
        }
        val response = executeJsonRequest(
            method = "POST",
            baseUrl = normalizedBaseUrl,
            path = path,
            requestBody = request.toString(),
            accessToken = null,
        )
        val json = JSONObject(response)
        val accessToken = json.optString("accessToken")
        val refreshToken = json.optString("refreshToken")
        val userEmail = json.optJSONObject("user")?.optString("email").orEmpty()
        require(accessToken.isNotBlank() && refreshToken.isNotBlank() && userEmail.isNotBlank()) {
            "User auth returned incomplete credentials."
        }

        val nowMs = System.currentTimeMillis()
        cachedAccessToken = accessToken
        cachedAccessExpiresAtMs = nowMs + 11L * 60L * 60L * 1000L
        cachedAccessBaseUrl = normalizedBaseUrl
        prefs.edit()
            .putString(PREF_BASE_URL, normalizedBaseUrl)
            .putString(PREF_USER_EMAIL, userEmail)
            .putString(PREF_USER_REFRESH_TOKEN, refreshToken)
            .putLong(PREF_USER_REFRESH_EXPIRES_AT_MS, nowMs + 29L * 24L * 60L * 60L * 1000L)
            .apply()
        return ControlPlaneAuthState("user", userEmail, userAuthenticated = true)
    }

    private fun runNatProbe(
        sessionId: String,
        probeToken: String,
        probeHost: String,
        probePort: Int,
        role: String,
    ): NatProbeEcho? {
        val socket = DatagramSocket()
        socket.soTimeout = 2000
        return try {
            val request = JSONObject().apply {
                put("kind", "nat_probe")
                put("sessionId", sessionId)
                put("probeToken", probeToken)
                put("role", role)
            }.toString().toByteArray(StandardCharsets.UTF_8)
            val target = DatagramPacket(request, request.size, InetAddress.getByName(probeHost), probePort)
            socket.send(target)

            val responseBuffer = ByteArray(1024)
            val responsePacket = DatagramPacket(responseBuffer, responseBuffer.size)
            socket.receive(responsePacket)
            val response = JSONObject(String(responsePacket.data, 0, responsePacket.length, StandardCharsets.UTF_8))
            if (!response.optString("kind").equals("nat_probe_ack", ignoreCase = true) ||
                response.optString("sessionId") != sessionId ||
                response.optString("probeToken") != probeToken
            ) {
                return null
            }

            val observedAddress = response.optString("observedAddress")
            val observedPort = response.optInt("observedPort")
            if (observedAddress.isBlank() || observedPort !in 1..65535) {
                return null
            }

            NatProbeEcho(
                observedAddress = observedAddress,
                observedPort = observedPort,
                localAddress = socket.localAddress?.hostAddress,
                localPort = socket.localPort.takeIf { it > 0 },
            )
        } catch (_: Throwable) {
            null
        } finally {
            socket.close()
        }
    }

    private fun parseLease(json: JSONObject): ControlPlaneSessionLease {
        val receiverEndpoint = json.optJSONObject("receiverEndpoint")
        return ControlPlaneSessionLease(
            sessionId = json.optString("sessionId"),
            sessionToken = json.optString("sessionToken"),
            hostId = json.optString("hostId"),
            hostDisplayName = json.optString("hostDisplayName"),
            status = json.opt("status")?.toString().orEmpty(),
            routeKind = json.optString("routeKind"),
            routeState = json.optString("routeState").ifBlank { json.optString("routeKind") },
            routeVersion = json.optInt("routeVersion", 0).coerceAtLeast(0),
            sessionHealth = json.optString("sessionHealth").ifBlank { "syncing" },
            sessionHealthReason = json.optString("sessionHealthReason").ifBlank { "unspecified" },
            routeActionHint = json.optString("routeActionHint").ifBlank { "wait_for_telemetry" },
            routeActionReason = json.optString("routeActionReason").ifBlank { "unspecified" },
            routeFallbackReadyDurationSeconds = json.optInt("routeFallbackReadyDurationSeconds", 0).coerceAtLeast(0),
            routeRecoveryReadyDurationSeconds = json.optInt("routeRecoveryReadyDurationSeconds", 0).coerceAtLeast(0),
            recommendedSyncDelaySeconds = json.optInt("recommendedSyncDelaySeconds", 10).coerceIn(5, 60),
            transportLossLevel = json.optString("transportLossLevel").ifBlank { "unknown" },
            transportAnomalyKind = json.optString("transportAnomalyKind").ifBlank { "unknown" },
            transportAnomalyReason = json.optString("transportAnomalyReason").ifBlank { "unspecified" },
            transportAnomalyConfidence = json.optString("transportAnomalyConfidence").ifBlank { "low" },
            receiverTelemetryAgeSeconds = json.optInt("receiverTelemetryAgeSeconds", -1),
            senderTelemetryAgeSeconds = json.optInt("senderTelemetryAgeSeconds", -1),
            lastRouteActionKind = json.optString("lastRouteActionKind").ifBlank { null },
            lastRouteActionReason = json.optString("lastRouteActionReason").ifBlank { null },
            lastRouteActionActor = json.optString("lastRouteActionActor").ifBlank { null },
            lastRouteActionUtc = json.optString("lastRouteActionUtc").ifBlank { null },
            routeRecoveryCount = json.optInt("routeRecoveryCount", 0).coerceAtLeast(0),
            routeRecoveryCooldownSeconds = json.optInt("routeRecoveryCooldownSeconds", 0).coerceAtLeast(0),
            routeFallbackCount = json.optInt("routeFallbackCount", 0).coerceAtLeast(0),
            routeFallbackCooldownSeconds = json.optInt("routeFallbackCooldownSeconds", 0).coerceAtLeast(0),
            codecPreference = json.optString("codecPreference").takeIf { it.isNotBlank() },
            relayAddress = json.optJSONObject("relayEndpoint")?.optString("host")?.takeIf { it.isNotBlank() },
            relayPort = json.optJSONObject("relayEndpoint")?.optInt("port")?.takeIf { it > 0 },
            relayRegion = json.optString("relayRegion").takeIf { it.isNotBlank() },
            probeAddress = json.optJSONObject("probeEndpoint")?.optString("host")?.takeIf { it.isNotBlank() },
            probePort = json.optJSONObject("probeEndpoint")?.optInt("port")?.takeIf { it > 0 },
            probeToken = json.optString("probeToken"),
            natStatus = json.optString("natStatus").ifBlank { "probe_unavailable" },
            hostNatProbeAgeSeconds = json.optInt("hostNatProbeAgeSeconds", -1),
            clientNatProbeAgeSeconds = json.optInt("clientNatProbeAgeSeconds", -1),
            natProbeFresh = json.optBoolean("natProbeFresh", false),
            receiverAddress = receiverEndpoint?.optString("host")?.takeIf { it.isNotBlank() },
            receiverPort = receiverEndpoint?.optInt("port")?.takeIf { it > 0 },
        )
    }

    private fun parseConnectInstructions(json: JSONObject): ControlPlaneConnectInstructions {
        val streamEndpoint = json.optJSONObject("streamEndpoint")
            ?: throw IllegalStateException("Connect instructions do not include stream endpoint.")
        return ControlPlaneConnectInstructions(
            sessionId = json.optString("sessionId"),
            hostId = json.optString("hostId"),
            hostDisplayName = json.optString("hostDisplayName"),
            status = json.opt("status")?.toString().orEmpty(),
            routeKind = json.optString("routeKind"),
            routeState = json.optString("routeState").ifBlank { json.optString("routeKind") },
            routeVersion = json.optInt("routeVersion", 0).coerceAtLeast(0),
            sessionHealth = json.optString("sessionHealth").ifBlank { "syncing" },
            sessionHealthReason = json.optString("sessionHealthReason").ifBlank { "unspecified" },
            routeActionHint = json.optString("routeActionHint").ifBlank { "wait_for_telemetry" },
            routeActionReason = json.optString("routeActionReason").ifBlank { "unspecified" },
            routeFallbackReadyDurationSeconds = json.optInt("routeFallbackReadyDurationSeconds", 0).coerceAtLeast(0),
            routeRecoveryReadyDurationSeconds = json.optInt("routeRecoveryReadyDurationSeconds", 0).coerceAtLeast(0),
            recommendedSyncDelaySeconds = json.optInt("recommendedSyncDelaySeconds", 10).coerceIn(5, 60),
            transportLossLevel = json.optString("transportLossLevel").ifBlank { "unknown" },
            transportAnomalyKind = json.optString("transportAnomalyKind").ifBlank { "unknown" },
            transportAnomalyReason = json.optString("transportAnomalyReason").ifBlank { "unspecified" },
            transportAnomalyConfidence = json.optString("transportAnomalyConfidence").ifBlank { "low" },
            receiverTelemetryAgeSeconds = json.optInt("receiverTelemetryAgeSeconds", -1),
            senderTelemetryAgeSeconds = json.optInt("senderTelemetryAgeSeconds", -1),
            lastRouteActionKind = json.optString("lastRouteActionKind").ifBlank { null },
            lastRouteActionReason = json.optString("lastRouteActionReason").ifBlank { null },
            lastRouteActionActor = json.optString("lastRouteActionActor").ifBlank { null },
            lastRouteActionUtc = json.optString("lastRouteActionUtc").ifBlank { null },
            routeRecoveryCount = json.optInt("routeRecoveryCount", 0).coerceAtLeast(0),
            routeRecoveryCooldownSeconds = json.optInt("routeRecoveryCooldownSeconds", 0).coerceAtLeast(0),
            routeFallbackCount = json.optInt("routeFallbackCount", 0).coerceAtLeast(0),
            routeFallbackCooldownSeconds = json.optInt("routeFallbackCooldownSeconds", 0).coerceAtLeast(0),
            streamHost = streamEndpoint.optString("host"),
            streamPort = streamEndpoint.optInt("port"),
            relayHost = json.optJSONObject("relayEndpoint")?.optString("host")?.takeIf { it.isNotBlank() },
            relayPort = json.optJSONObject("relayEndpoint")?.optInt("port")?.takeIf { it > 0 },
            relayRegion = json.optString("relayRegion").takeIf { it.isNotBlank() },
            probeHost = json.optJSONObject("probeEndpoint")?.optString("host")?.takeIf { it.isNotBlank() },
            probePort = json.optJSONObject("probeEndpoint")?.optInt("port")?.takeIf { it > 0 },
            probeToken = json.optString("probeToken"),
            natStatus = json.optString("natStatus").ifBlank { "probe_unavailable" },
            receiverRegistered = json.optBoolean("receiverRegistered", false),
            hostReady = json.optBoolean("hostReady", false),
            hostNatProbeAgeSeconds = json.optInt("hostNatProbeAgeSeconds", -1),
            clientNatProbeAgeSeconds = json.optInt("clientNatProbeAgeSeconds", -1),
            natProbeFresh = json.optBoolean("natProbeFresh", false),
        )
    }

    private fun parseRoutePolicy(json: JSONObject): ControlPlaneRoutePolicy {
        return ControlPlaneRoutePolicy(
            sessionId = json.optString("sessionId"),
            hostId = json.optString("hostId"),
            routeKind = json.optString("routeKind"),
            routeState = json.optString("routeState").ifBlank { json.optString("routeKind") },
            routeVersion = json.optInt("routeVersion", 0).coerceAtLeast(0),
            sessionHealth = json.optString("sessionHealth").ifBlank { "syncing" },
            sessionHealthReason = json.optString("sessionHealthReason").ifBlank { "unspecified" },
            routeActionHint = json.optString("routeActionHint").ifBlank { "wait_for_telemetry" },
            routeActionReason = json.optString("routeActionReason").ifBlank { "unspecified" },
            recommendedSyncDelaySeconds = json.optInt("recommendedSyncDelaySeconds", 10).coerceIn(5, 60),
            transportLossLevel = json.optString("transportLossLevel").ifBlank { "unknown" },
            transportAnomalyKind = json.optString("transportAnomalyKind").ifBlank { "unknown" },
            transportAnomalyReason = json.optString("transportAnomalyReason").ifBlank { "unspecified" },
            transportAnomalyConfidence = json.optString("transportAnomalyConfidence").ifBlank { "low" },
            actionableAnomaly = json.optBoolean("actionableAnomaly", false),
            highConfidenceAnomaly = json.optBoolean("highConfidenceAnomaly", false),
            fallbackWarmupSeconds = json.optInt("fallbackWarmupSeconds", 8).coerceAtLeast(0),
            fallbackReadyDurationSeconds = json.optInt("fallbackReadyDurationSeconds", 0).coerceAtLeast(0),
            fallbackReady = json.optBoolean("fallbackReady", false),
            recoveryWarmupSeconds = json.optInt("recoveryWarmupSeconds", 12).coerceAtLeast(0),
            recoveryReadyDurationSeconds = json.optInt("recoveryReadyDurationSeconds", 0).coerceAtLeast(0),
            recoveryReady = json.optBoolean("recoveryReady", false),
            fallbackCooldownSeconds = json.optInt("fallbackCooldownSeconds", 0).coerceAtLeast(0),
            recoveryCooldownSeconds = json.optInt("recoveryCooldownSeconds", 0).coerceAtLeast(0),
            receiverTelemetryAgeSeconds = json.optInt("receiverTelemetryAgeSeconds", -1),
            senderTelemetryAgeSeconds = json.optInt("senderTelemetryAgeSeconds", -1),
            natStatus = json.optString("natStatus").ifBlank { "probe_unavailable" },
            hostNatProbeAgeSeconds = json.optInt("hostNatProbeAgeSeconds", -1),
            clientNatProbeAgeSeconds = json.optInt("clientNatProbeAgeSeconds", -1),
            natProbeFresh = json.optBoolean("natProbeFresh", false),
        )
    }

    private fun executeJsonRequest(
        method: String,
        baseUrl: String,
        path: String,
        requestBody: String?,
        accessToken: String?,
    ): String {
        val normalizedBaseUrl = baseUrl.trim().trimEnd('/')
        require(normalizedBaseUrl.isNotBlank()) { "Control plane URL is required." }

        val connection = (URL("$normalizedBaseUrl$path").openConnection() as HttpURLConnection).apply {
            requestMethod = method
            connectTimeout = 4_000
            readTimeout = 4_000
            setRequestProperty("Accept", "application/json")
            if (requestBody != null) {
                doOutput = true
                setRequestProperty("Content-Type", "application/json; charset=utf-8")
            }
            if (!accessToken.isNullOrBlank()) {
                setRequestProperty("Authorization", "Bearer $accessToken")
            }
        }

        return try {
            if (requestBody != null) {
                connection.outputStream.use { output ->
                    output.write(requestBody.toByteArray(StandardCharsets.UTF_8))
                }
            }

            val statusCode = connection.responseCode
            val body = readResponseBody(connection, statusCode < 400)
            if (statusCode !in 200..299) {
                throw IllegalStateException(extractApiError(body).ifBlank { "HTTP $statusCode" })
            }
            body
        } finally {
            connection.disconnect()
        }
    }

    private fun readResponseBody(connection: HttpURLConnection, success: Boolean): String {
        val stream = if (success) connection.inputStream else connection.errorStream
        if (stream == null) {
            return ""
        }
        return BufferedReader(InputStreamReader(stream, StandardCharsets.UTF_8)).use { reader ->
            buildString {
                var line: String?
                while (reader.readLine().also { line = it } != null) {
                    append(line)
                }
            }
        }
    }

    private fun extractApiError(body: String): String {
        return runCatching {
            JSONObject(body).optString("message")
        }.getOrDefault(body)
    }
}
