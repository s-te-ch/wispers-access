package dev.wispers.access.android

import android.app.Activity
import android.app.Application
import android.os.Bundle
import android.os.SystemClock
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.atomic.AtomicInteger
import javax.inject.Inject
import javax.inject.Singleton

/**
 * Tracks whether any activity is currently started, i.e. the app is visible to
 * the user, and notifies listeners when the app returns to the foreground.
 * Registered by [WispersAccessApp] at process start.
 */
@Singleton
class ForegroundTracker @Inject constructor() : Application.ActivityLifecycleCallbacks {

    /** Called on the main thread when the app returns to the foreground. */
    fun interface OnForegroundListener {
        fun onForeground(backgroundedForMs: Long)
    }

    private val startedCount = AtomicInteger(0)
    private val listeners = CopyOnWriteArrayList<OnForegroundListener>()

    // Monotonic timestamp of the last drop to zero started activities, or
    // NEVER_BACKGROUNDED before the first one (process start is not a "return").
    @Volatile
    private var backgroundedAt = NEVER_BACKGROUNDED

    val isForeground: Boolean
        get() = startedCount.get() > 0

    fun addOnForegroundListener(listener: OnForegroundListener) {
        listeners += listener
    }

    override fun onActivityStarted(activity: Activity) {
        if (startedCount.incrementAndGet() == 1 && backgroundedAt != NEVER_BACKGROUNDED) {
            val backgroundedFor = SystemClock.elapsedRealtime() - backgroundedAt
            for (listener in listeners) listener.onForeground(backgroundedFor)
        }
    }

    override fun onActivityStopped(activity: Activity) {
        if (startedCount.decrementAndGet() == 0) {
            backgroundedAt = SystemClock.elapsedRealtime()
        }
    }

    override fun onActivityCreated(activity: Activity, savedInstanceState: Bundle?) = Unit
    override fun onActivityResumed(activity: Activity) = Unit
    override fun onActivityPaused(activity: Activity) = Unit
    override fun onActivitySaveInstanceState(activity: Activity, outState: Bundle) = Unit
    override fun onActivityDestroyed(activity: Activity) = Unit

    private companion object {
        const val NEVER_BACKGROUNDED = -1L
    }
}
