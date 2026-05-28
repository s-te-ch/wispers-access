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
import androidx.activity.compose.setContent
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.net.toUri
import androidx.lifecycle.lifecycleScope
import dagger.hilt.android.AndroidEntryPoint
import dev.wispers.access.android.proxy.ProxyServer
import dev.wispers.access.android.storage.ShareId
import dev.wispers.access.android.storage.ShareRepository
import dev.wispers.access.android.ui.theme.WispersAccessTheme
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

    private var webView: WebView? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        val shareId = intent.data?.lastPathSegment?.let(::ShareId) ?: run {
            finish()
            return
        }

        setContent {
            WispersAccessTheme {
                ShareWebViewScreen(
                    url = "http://${shareId.value}.localhost:${proxyServer.port}/",
                    savedState = savedInstanceState,
                    onWebViewReady = { webView = it },
                )
            }
        }

        onBackPressedDispatcher.addCallback(this, object : OnBackPressedCallback(true) {
            override fun handleOnBackPressed() {
                val wv = webView
                if (wv != null && wv.canGoBack()) {
                    wv.goBack()
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
        webView?.saveState(outState)
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

@Composable
private fun ShareWebViewScreen(
    url: String,
    savedState: Bundle?,
    onWebViewReady: (WebView) -> Unit,
) {
    var loading by remember { mutableStateOf(true) }

    Box(modifier = Modifier.fillMaxSize()) {
        AndroidView(
            modifier = Modifier.fillMaxSize(),
            factory = { context ->
                WebView(context).apply {
                    settings.javaScriptEnabled = true
                    settings.domStorageEnabled = true
                    webChromeClient = WebChromeClient()
                    webViewClient = object : WebViewClient() {
                        override fun onPageCommitVisible(view: WebView?, url: String?) {
                            loading = false
                        }
                    }
                    if (savedState != null) {
                        restoreState(savedState)
                    } else {
                        loadUrl(url)
                    }
                    onWebViewReady(this)
                }
            },
        )
        if (loading) {
            ConnectingOverlay(modifier = Modifier.fillMaxSize())
        }
    }
}

@Composable
private fun ConnectingOverlay(modifier: Modifier = Modifier) {
    Box(
        modifier = modifier.background(MaterialTheme.colorScheme.background),
        contentAlignment = Alignment.Center,
    ) {
        Column(
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            CircularProgressIndicator()
            Text("Connecting…", style = MaterialTheme.typography.bodyLarge)
        }
    }
}
