package dev.wispers.access.android.proxy

import android.util.Log
import dev.wispers.access.android.ForegroundTracker
import dev.wispers.access.android.SdkHolder
import javax.inject.Inject
import javax.inject.Singleton
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch

/**
 * Has the SDK check its cached connections when the app returns to the
 * foreground after a while in the background.
 *
 * While backgrounded, Android eventually freezes the process: keepalives stop
 * and the NAT path under an idle connection expires, but the connection
 * learns nothing — the first exchange on it would stall until a timeout. The
 * SDK's check probes each cached connection at the QUIC layer under a short
 * deadline, drops the dead ones and redials those in use, so the next tap
 * finds a working connection. A quick trip to the recents screen doesn't
 * qualify: keepalives run until the process is actually frozen, so
 * connections survive short background stints untouched.
 */
@Singleton
class ResumeMonitor @Inject constructor(
    private val foregroundTracker: ForegroundTracker,
    private val sdk: SdkHolder,
) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)

    fun start() {
        foregroundTracker.addOnForegroundListener { backgroundedForMs ->
            if (backgroundedForMs >= SUSPECT_AFTER_BACKGROUND_MS) {
                Log.i(TAG, "Foregrounded after ${backgroundedForMs / 1000}s, checking cached connections")
                val client = sdk.client ?: return@addOnForegroundListener
                scope.launch { client.checkConnections() }
            }
        }
    }

    private companion object {
        const val TAG = "ResumeMonitor"

        // Two QUIC keepalive intervals (15s each): a connection that missed at
        // most one keepalive is almost certainly still alive, so don't spend a
        // probe on quick app switches.
        const val SUSPECT_AFTER_BACKGROUND_MS = 30_000L
    }
}
