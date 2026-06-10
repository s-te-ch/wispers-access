package dev.wispers.access.android

import android.app.Application
import android.content.pm.ApplicationInfo
import android.webkit.WebView
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
        // Debug builds: expose WebViews to desktop DevTools via chrome://inspect.
        if (applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE != 0) {
            WebView.setWebContentsDebuggingEnabled(true)
        }
        registerActivityLifecycleCallbacks(foregroundTracker)
        proxyServer.start()
        networkMonitor.start()
    }
}
