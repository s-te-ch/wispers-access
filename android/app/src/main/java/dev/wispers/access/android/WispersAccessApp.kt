package dev.wispers.access.android

import android.app.Application
import dagger.hilt.android.HiltAndroidApp
import dev.wispers.access.android.proxy.NetworkMonitor
import dev.wispers.access.android.proxy.ProxyServer
import javax.inject.Inject

@HiltAndroidApp
class WispersAccessApp : Application() {

    @Inject
    lateinit var proxyServer: ProxyServer

    @Inject
    lateinit var networkMonitor: NetworkMonitor

    @Inject
    lateinit var foregroundTracker: ForegroundTracker

    override fun onCreate() {
        super.onCreate()
        registerActivityLifecycleCallbacks(foregroundTracker)
        proxyServer.start()
        networkMonitor.start()
    }
}
