package dev.wispers.access.android

import android.app.Application
import dagger.hilt.android.HiltAndroidApp
import dev.wispers.access.android.proxy.ProxyServer
import javax.inject.Inject

@HiltAndroidApp
class WispersAccessApp : Application() {

    @Inject
    lateinit var proxyServer: ProxyServer

    override fun onCreate() {
        super.onCreate()
        proxyServer.start()
    }
}
