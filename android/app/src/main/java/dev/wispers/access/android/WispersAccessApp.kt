package dev.wispers.access.android

import android.app.Application
import android.content.pm.ApplicationInfo
import android.webkit.WebView
import dagger.hilt.android.HiltAndroidApp
import dev.wispers.access.android.proxy.NetworkMonitor
import dev.wispers.access.android.proxy.ProxyHolder
import dev.wispers.access.android.proxy.ResumeMonitor
import dev.wispers.access.android.storage.SecretDao
import dev.wispers.access.android.storage.SqlCipherSecretStore
import dev.wispers.access.sdk.Client
import dev.wispers.access.sdk.ClientConfig
import dev.wispers.access.sdk.LogLevel
import dev.wispers.access.sdk.installLogSink
import java.io.File
import javax.inject.Inject

@HiltAndroidApp
class WispersAccessApp : Application() {

    @Inject
    lateinit var sdk: SdkHolder

    @Inject
    internal lateinit var secretDao: SecretDao

    @Inject
    lateinit var proxy: ProxyHolder

    @Inject
    lateinit var networkMonitor: NetworkMonitor

    @Inject
    lateinit var resumeMonitor: ResumeMonitor

    @Inject
    lateinit var foregroundTracker: ForegroundTracker

    override fun onCreate() {
        super.onCreate()
        // Debug builds: expose WebViews to desktop DevTools via chrome://inspect.
        if (applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE != 0) {
            WebView.setWebContentsDebuggingEnabled(true)
        }
        // The SDK's lines into logcat, once per process.
        installLogSink(SdkLog(), LogLevel.INFO)
        // The SDK's client: its store under our files, its secrets in the
        // SQLCipher database, and the roster told of every change.
        sdk.client = Client(
            ClientConfig(
                dataDir = File(filesDir, "sdk").path,
                secrets = SqlCipherSecretStore(secretDao),
                observer = sdk,
            )
        )
        registerActivityLifecycleCallbacks(foregroundTracker)
        proxy.start()
        networkMonitor.start()
        resumeMonitor.start()
    }
}
