package com.everty.evertygame.touch

import android.accessibilityservice.AccessibilityService
import android.graphics.Rect
import android.view.accessibility.AccessibilityEvent

class TouchLatencySprintAccessibilityService : AccessibilityService() {
    override fun onAccessibilityEvent(event: AccessibilityEvent?) {
        val eventType = event?.eventType ?: return
        val bounds = event.source?.let { source ->
            Rect().also(source::getBoundsInScreen)
        }
        when (eventType) {
            AccessibilityEvent.TYPE_TOUCH_INTERACTION_START,
            AccessibilityEvent.TYPE_VIEW_CLICKED,
            -> TouchLatencySprintController.trigger(TouchLatencySprintController.PulseKind.TAP, bounds)

            AccessibilityEvent.TYPE_VIEW_LONG_CLICKED,
            -> TouchLatencySprintController.trigger(TouchLatencySprintController.PulseKind.LONG_PRESS, bounds)

            AccessibilityEvent.TYPE_VIEW_SCROLLED,
            -> TouchLatencySprintController.trigger(TouchLatencySprintController.PulseKind.SCROLL, bounds)

            AccessibilityEvent.TYPE_GESTURE_DETECTION_START,
            -> TouchLatencySprintController.trigger(TouchLatencySprintController.PulseKind.GESTURE, bounds)
        }
    }

    override fun onInterrupt() = Unit
}
