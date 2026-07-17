import UIKit
import WebKit

/// Harvests a browsed site's best icon and reports it via `onIcon`. A
/// `WKScriptMessageHandler` that receives the page's pick — chosen by injected
/// same-origin JS run on every page-finish — and hands back validated image bytes
/// plus their rank. Ported from the Android `ShareActivity` harvester: the JS
/// prefers manifest-maskable (4) > manifest (3) > apple-touch-icon (2) >
/// favicon (1), fetches the winner with credentials (so it flows through the
/// loopback proxy with the same cookies/identity as the page), and only returns
/// an actual image on an OK response. Native discards anything that doesn't
/// out-rank the cached icon (see `ShareIconStore`).
final class IconHarvester: NSObject, WKScriptMessageHandler {
    static let messageName = "waIcon"

    private let shareID: ShareID
    private let onIcon: (ShareID, Data, Int) -> Void

    init(shareID: ShareID, onIcon: @escaping (ShareID, Data, Int) -> Void) {
        self.shareID = shareID
        self.onIcon = onIcon
    }

    /// Runs the picker in `webView`; call on each navigation finish.
    func harvest(in webView: WKWebView) {
        webView.evaluateJavaScript(Self.script, completionHandler: nil)
    }

    func userContentController(
        _ controller: WKUserContentController,
        didReceive message: WKScriptMessage
    ) {
        guard let body = message.body as? [String: Any],
            let rank = body["rank"] as? Int, rank > 0,
            let dataURL = body["dataUrl"] as? String,
            let bytes = Self.decodeDataURL(dataURL),
            // Only accept bytes we can actually render, so an undecodable icon
            // (e.g. a raw .ico favicon) can't claim a rung and block a good one.
            UIImage(data: bytes) != nil
        else { return }
        onIcon(shareID, bytes, rank)
    }

    /// Decodes a `data:` URL's base64 payload, or nil if malformed.
    private static func decodeDataURL(_ dataURL: String) -> Data? {
        guard let comma = dataURL.firstIndex(of: ",") else { return nil }
        return Data(base64Encoded: String(dataURL[dataURL.index(after: comma)...]))
    }

    private static let script = """
    (function () {
      function abs(u) { try { return new URL(u, location.href).href; } catch (e) { return null; } }
      function done(rank, dataUrl) {
        try { window.webkit.messageHandlers.waIcon.postMessage({ rank: rank, dataUrl: dataUrl || '' }); } catch (e) {}
      }
      async function run() {
        var best = null;
        var ml = document.querySelector('link[rel~="manifest"]');
        if (ml && ml.href) {
          try {
            var m = await (await fetch(ml.href, { credentials: 'include' })).json();
            (m.icons || []).forEach(function (ic) {
              var sizes = String(ic.sizes || '').split(/\\s+/).map(function (s) { return parseInt(s) || 0; });
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
          // fetch() resolves on 4xx/5xx too, so guard on resp.ok and an image
          // content-type — otherwise a missing /favicon.ico hands back the
          // server's HTML error page as a (rank-1, undecodable) "icon".
          var resp = await fetch(best.url, { credentials: 'include' });
          if (!resp.ok) { done(0); return; }
          var blob = await resp.blob();
          if (!/^image\\//.test(blob.type)) { done(0); return; }
          var reader = new FileReader();
          reader.onloadend = function () { done(best.rank, reader.result); };
          reader.onerror = function () { done(0); };
          reader.readAsDataURL(blob);
        } catch (e) { done(0); }
      }
      run();
    })();
    """
}
