package dev.wispers.access.android

import android.app.ActivityManager
import android.content.Context
import android.content.Intent
import android.os.Bundle
import android.webkit.WebChromeClient
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.activity.ComponentActivity
import androidx.activity.OnBackPressedCallback
import androidx.core.net.toUri
import androidx.lifecycle.lifecycleScope
import dagger.hilt.android.AndroidEntryPoint
import dev.wispers.access.android.proxy.ProxyServer
import dev.wispers.access.android.storage.ShareId
import dev.wispers.access.android.storage.ShareRepository
import javax.inject.Inject
import kotlinx.coroutines.launch

/**
 * Hosts a single WebView pointed at the local proxy for one share.
 *
 * Launched via [launch] with `Intent.FLAG_ACTIVITY_NEW_DOCUMENT` + a unique data URI
 * per share, which combined with `documentLaunchMode="intoExisting"` in the manifest
 * gives each share its own task entry in Recents and brings the existing one forward
 * on re-launch.
 */
@AndroidEntryPoint
class ShareActivity : ComponentActivity() {

    @Inject lateinit var repo: ShareRepository
    @Inject lateinit var proxyServer: ProxyServer

    private lateinit var webView: WebView

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        val shareId = intent.data?.lastPathSegment?.let(::ShareId) ?: run {
            finish()
            return
        }

        webView = WebView(this).apply {
            settings.javaScriptEnabled = true
            settings.domStorageEnabled = true
            webViewClient = WebViewClient()
            webChromeClient = WebChromeClient()
        }
        setContentView(webView)

        if (savedInstanceState == null) {
            webView.loadUrl("http://${shareId.value}.localhost:${proxyServer.port}/")
        } else {
            webView.restoreState(savedInstanceState)
        }

        onBackPressedDispatcher.addCallback(this, object : OnBackPressedCallback(true) {
            override fun handleOnBackPressed() {
                if (webView.canGoBack()) {
                    webView.goBack()
                } else {
                    isEnabled = false
                    onBackPressedDispatcher.onBackPressed()
                }
            }
        })

        lifecycleScope.launch {
            val nickname = repo.getShare(shareId)?.nickname?.ifBlank { null }
            val label = nickname ?: getString(R.string.app_name)
            setTaskDescription(ActivityManager.TaskDescription.Builder().setLabel(label).build())
        }
    }

    override fun onSaveInstanceState(outState: Bundle) {
        super.onSaveInstanceState(outState)
        webView.saveState(outState)
    }

    companion object {
        fun launch(context: Context, shareId: ShareId) {
            val intent = Intent(context, ShareActivity::class.java).apply {
                data = "wispers-access://share/${shareId.value}".toUri()
                addFlags(Intent.FLAG_ACTIVITY_NEW_DOCUMENT)
            }
            context.startActivity(intent)
        }
    }
}
