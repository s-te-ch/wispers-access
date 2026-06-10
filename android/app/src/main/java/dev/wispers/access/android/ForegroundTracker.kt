package dev.wispers.access.android

import android.app.Activity
import android.app.Application
import android.os.Bundle
import java.util.concurrent.atomic.AtomicInteger
import javax.inject.Inject
import javax.inject.Singleton

/**
 * Tracks whether any activity is currently started, i.e. the app is visible to
 * the user. Registered by [WispersAccessApp] at process start.
 */
@Singleton
class ForegroundTracker @Inject constructor() : Application.ActivityLifecycleCallbacks {

    private val startedCount = AtomicInteger(0)

    val isForeground: Boolean
        get() = startedCount.get() > 0

    override fun onActivityStarted(activity: Activity) {
        startedCount.incrementAndGet()
    }

    override fun onActivityStopped(activity: Activity) {
        startedCount.decrementAndGet()
    }

    override fun onActivityCreated(activity: Activity, savedInstanceState: Bundle?) = Unit
    override fun onActivityResumed(activity: Activity) = Unit
    override fun onActivityPaused(activity: Activity) = Unit
    override fun onActivitySaveInstanceState(activity: Activity, outState: Bundle) = Unit
    override fun onActivityDestroyed(activity: Activity) = Unit
}
