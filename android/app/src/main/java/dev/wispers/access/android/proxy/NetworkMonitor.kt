package dev.wispers.access.android.proxy

import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.util.Log
import dagger.hilt.android.qualifiers.ApplicationContext
import javax.inject.Inject
import javax.inject.Singleton
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch

/**
 * Evicts cached QUIC connections when the device's default network changes.
 *
 * This is what Chrome does on handover: the OS push signal beats waiting for
 * blackholed connections to hit their request timeouts, so the first request
 * after a Wi-Fi/cellular switch reconnects immediately instead of stalling.
 */
@Singleton
class NetworkMonitor @Inject constructor(
    @param:ApplicationContext private val context: Context,
    private val sessionManager: SessionManager,
) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)

    // Callbacks arrive serially on ConnectivityManager's handler thread.
    private var lastNetwork: Network? = null

    fun start() {
        val cm = context.getSystemService(ConnectivityManager::class.java)
        cm.registerDefaultNetworkCallback(object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) {
                val previous = lastNetwork
                lastNetwork = network
                if (previous != null && previous != network) {
                    Log.i(TAG, "Default network changed, evicting cached connections")
                    scope.launch { sessionManager.evictAll() }
                }
            }
        })
    }

    private companion object {
        const val TAG = "NetworkMonitor"
    }
}
