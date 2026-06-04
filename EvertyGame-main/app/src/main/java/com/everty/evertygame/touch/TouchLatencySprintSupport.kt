package com.everty.evertygame.touch

import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.provider.Settings

object TouchLatencySprintSupport {
    fun isServiceEnabled(context: Context): Boolean {
        val accessibilityEnabled = Settings.Secure.getInt(
            context.contentResolver,
            Settings.Secure.ACCESSIBILITY_ENABLED,
            0,
        ) == 1
        if (!accessibilityEnabled) {
            return false
        }

        val enabledServices = Settings.Secure.getString(
            context.contentResolver,
            Settings.Secure.ENABLED_ACCESSIBILITY_SERVICES,
        ).orEmpty()
        if (enabledServices.isBlank()) {
            return false
        }

        val componentName = ComponentName(context, TouchLatencySprintAccessibilityService::class.java)
        val expectedNames = setOf(
            componentName.flattenToString(),
            componentName.flattenToShortString(),
        )

        return enabledServices
            .split(':')
            .any { candidate ->
                expectedNames.any { expected ->
                    candidate.equals(expected, ignoreCase = true)
                }
            }
    }

    fun openAccessibilitySettings(context: Context) {
        context.startActivity(
            Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS).apply {
                addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            },
        )
    }
}
