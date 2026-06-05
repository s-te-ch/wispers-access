package dev.wispers.access.android

import android.app.ActivityManager
import android.content.Context
import android.content.Intent
import android.os.Bundle
import android.util.Base64
import android.webkit.JavascriptInterface
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
import androidx.core.content.pm.ShortcutInfoCompat
import androidx.core.content.pm.ShortcutManagerCompat
import androidx.core.net.toUri
import androidx.lifecycle.lifecycleScope
import dagger.hilt.android.AndroidEntryPoint
import dev.wispers.access.android.proxy.ProxyServer
import dev.wispers.access.android.storage.ShareId
import dev.wispers.access.android.storage.ShareRepository
import dev.wispers.access.android.ui.theme.WispersAccessTheme
import javax.inject.Inject
import kotlinx.coroutines.launch
import org.json.JSONObject

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
                    onIconJson = { json -> handleHarvestedIcon(shareId, json) },
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

    /**
     * Receives an icon-harvest result from the page (via [IconBridge]). Stores the
     * icon only if it out-ranks what's cached, then refreshes any pinned shortcut in
     * place. Best-effort — a failure just leaves the existing icon untouched.
     */
    private fun handleHarvestedIcon(shareId: ShareId, json: String) {
        lifecycleScope.launch {
            val obj = runCatching { JSONObject(json) }.getOrNull() ?: return@launch
            val rank = obj.optInt("rank", 0)
            val dataUrl = obj.optString("dataUrl", "")
            if (rank <= 0 || dataUrl.isEmpty()) return@launch
            if (rank <= repo.currentIconRank(shareId)) return@launch
            val bytes = decodeDataUrl(dataUrl) ?: return@launch
            repo.updateIcon(shareId, bytes, rank)
            val nickname = repo.getShare(shareId)?.nickname.orEmpty()
            refreshShortcut(this@ShareActivity, shareId, nickname, bytes)
        }
    }

    companion object {
        fun launch(context: Context, shareId: ShareId) {
            context.startActivity(intentFor(context, shareId))
        }

        /**
         * Intent that opens [shareId]'s WebView. The unique data URI doubles as the
         * per-share Recents task key (see `documentLaunchMode="intoExisting"`); the
         * ACTION_VIEW is required because pinned-shortcut intents must carry an action.
         */
        fun intentFor(context: Context, shareId: ShareId): Intent =
            Intent(context, ShareActivity::class.java).apply {
                action = Intent.ACTION_VIEW
                data = "wispers-access://share/${shareId.value}".toUri()
                addFlags(Intent.FLAG_ACTIVITY_NEW_DOCUMENT)
            }
    }
}

private fun buildShortcut(
    context: Context,
    shareId: ShareId,
    nickname: String,
    iconPng: ByteArray?,
): ShortcutInfoCompat {
    val label = nickname.ifBlank { "Untitled share" }
    return ShortcutInfoCompat.Builder(context, "share-${shareId.value}")
        .setShortLabel(label)
        .setLongLabel(label)
        .setIcon(shareIcon(context, nickname, iconPng))
        .setIntent(ShareActivity.intentFor(context, shareId))
        .build()
}

/**
 * Requests that the launcher pin a home-screen shortcut that opens [shareId].
 * Returns false if the current launcher doesn't support pinning shortcuts.
 */
fun addToHomescreen(
    context: Context,
    shareId: ShareId,
    nickname: String,
    iconPng: ByteArray?,
): Boolean {
    if (!ShortcutManagerCompat.isRequestPinShortcutSupported(context)) return false
    return ShortcutManagerCompat.requestPinShortcut(
        context,
        buildShortcut(context, shareId, nickname, iconPng),
        null,
    )
}

/**
 * Updates an already-pinned shortcut's icon/label in place. A no-op unless a
 * pinned or dynamic shortcut with this id exists, so it's safe to call always.
 */
fun refreshShortcut(context: Context, shareId: ShareId, nickname: String, iconPng: ByteArray?) {
    ShortcutManagerCompat.updateShortcuts(
        context,
        listOf(buildShortcut(context, shareId, nickname, iconPng)),
    )
}

/** Decodes a `data:` URL's base64 payload, or null if malformed. */
private fun decodeDataUrl(dataUrl: String): ByteArray? {
    val comma = dataUrl.indexOf(',')
    if (comma < 0) return null
    return runCatching { Base64.decode(dataUrl.substring(comma + 1), Base64.DEFAULT) }.getOrNull()
}

/** Bridge the page's JS uses to hand a harvested icon (JSON `{rank, dataUrl}`) to native code. */
private class IconBridge(private val onJson: (String) -> Unit) {
    @JavascriptInterface
    fun onIcon(json: String) = onJson(json)
}

/**
 * Same-origin JS that picks the best available site icon — manifest maskable (4) >
 * manifest (3) > apple-touch-icon (2) > favicon (1) — fetches it (with credentials,
 * so the proxy's identity header and any cookies apply and auth-gated icons
 * resolve), and hands it back as a data URL via [IconBridge]. It runs on every
 * page-finish; native discards anything that doesn't out-rank the cached icon.
 */
private const val HARVEST_JS = """
(function () {
  function abs(u) { try { return new URL(u, location.href).href; } catch (e) { return null; } }
  function done(rank, dataUrl) {
    try { WAIcon.onIcon(JSON.stringify({ rank: rank, dataUrl: dataUrl || '' })); } catch (e) {}
  }
  async function run() {
    var best = null;
    var ml = document.querySelector('link[rel~="manifest"]');
    if (ml && ml.href) {
      try {
        var m = await (await fetch(ml.href, { credentials: 'include' })).json();
        (m.icons || []).forEach(function (ic) {
          var sizes = String(ic.sizes || '').split(/\s+/).map(function (s) { return parseInt(s) || 0; });
          var size = Math.max.apply(null, [0].concat(sizes));
          var rank = String(ic.purpose || '').indexOf('maskable') >= 0 ? 4 : 3;
          if (!best || rank > best.rank || (rank === best.rank && size > best.size)) {
            best = { rank: rank, size: size, url: abs(ic.src) };
          }
        });
      } catch (e) {}
    }
    if (!best || best.rank < 2) {
      var at = document.querySelector('link[rel~="apple-touch-icon"], link[rel~="apple-touch-icon-precomposed"]');
      if (at && at.href) best = { rank: 2, size: 0, url: abs(at.href) };
    }
    if (!best) {
      var fav = document.querySelector('link[rel~="icon"]');
      var url = fav && fav.href ? abs(fav.href) : abs('/favicon.ico');
      if (url) best = { rank: 1, size: 0, url: url };
    }
    if (!best || !best.url) { done(0); return; }
    try {
      var blob = await (await fetch(best.url, { credentials: 'include' })).blob();
      var reader = new FileReader();
      reader.onloadend = function () { done(best.rank, reader.result); };
      reader.onerror = function () { done(0); };
      reader.readAsDataURL(blob);
    } catch (e) { done(0); }
  }
  run();
})();
"""

@Composable
private fun ShareWebViewScreen(
    url: String,
    savedState: Bundle?,
    onWebViewReady: (WebView) -> Unit,
    onIconJson: (String) -> Unit,
) {
    var loading by remember { mutableStateOf(true) }

    Box(modifier = Modifier.fillMaxSize()) {
        AndroidView(
            modifier = Modifier.fillMaxSize(),
            factory = { context ->
                WebView(context).apply {
                    settings.javaScriptEnabled = true
                    settings.domStorageEnabled = true
                    addJavascriptInterface(IconBridge(onIconJson), "WAIcon")
                    webChromeClient = WebChromeClient()
                    webViewClient = object : WebViewClient() {
                        override fun onPageCommitVisible(view: WebView?, url: String?) {
                            loading = false
                        }

                        override fun onPageFinished(view: WebView?, url: String?) {
                            view?.evaluateJavascript(HARVEST_JS, null)
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
