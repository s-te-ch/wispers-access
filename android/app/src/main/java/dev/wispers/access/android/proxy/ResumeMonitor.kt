package dev.wispers.access.android.proxy

import android.util.Log
import dev.wispers.access.android.ForegroundTracker
import javax.inject.Inject
import javax.inject.Singleton
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch

/**
 * Marks cached QUIC connections suspect when the app returns to the foreground
 * after a while in the background.
 *
 * While backgrounded, Android eventually freezes the process: keepalives stop
 * and the NAT/consent path under an idle connection expires, but the connection
 * object learns nothing — the first exchange on it would stall for the full
 * backstop deadline before failing. Marking it suspect turns that first
 * exchange into a short probe, which ProxyServer replays on a fresh connection
 * if it fails. A quick trip to the recents screen doesn't qualify: keepalives
 * run until the process is actually frozen, so connections survive short
 * background stints and stay on the normal deadline.
 */
@Singleton
class ResumeMonitor @Inject constructor(
    private val foregroundTracker: ForegroundTracker,
    private val sessionManager: SessionManager,
) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)

    fun start() {
        foregroundTracker.addOnForegroundListener { backgroundedForMs ->
            if (backgroundedForMs >= SUSPECT_AFTER_BACKGROUND_MS) {
                Log.i(TAG, "Foregrounded after ${backgroundedForMs / 1000}s, cached connections are suspect")
                scope.launch { sessionManager.markConnectionsSuspect() }
            }
        }
    }

    private companion object {
        const val TAG = "ResumeMonitor"

        // Two QUIC keepalive intervals (15s each): a connection that missed at
        // most one keepalive is almost certainly still alive, so don't punish
        // quick app switches with probe deadlines.
        const val SUSPECT_AFTER_BACKGROUND_MS = 30_000L
    }
}
