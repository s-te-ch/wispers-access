import UIKit
import WebKit
import WispersAccessSdk

/// Harvests a browsed site's best icon and reports it via `onIcon`. A
/// `WKScriptMessageHandler` that receives the page's pick — chosen by injected
/// same-origin JS run on every page-finish — and hands back validated image bytes
/// plus their rank. The same JS as Android's `BrowseActivity` harvester: it
/// prefers manifest-maskable (4) > manifest (3) > apple-touch-icon (2) >
/// favicon (1), fetches the winner with credentials (so it flows through the
/// loopback proxy with the same cookies/identity as the page), only accepts an
/// actual image on an OK response, and rasterises it to a PNG in the page, so
/// SVG and ICO favicons work too. Native discards anything that doesn't
/// out-rank the cached icon (see `ShareIconStore`).
final class IconHarvester: NSObject, WKScriptMessageHandler {
    static let messageName = "waIcon"

    private let key: SharedAppId
    private let onIcon: (SharedAppId, Data, Int) -> Void

    init(key: SharedAppId, onIcon: @escaping (SharedAppId, Data, Int) -> Void) {
        self.key = key
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
        onIcon(key, bytes, rank)
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
      // A PNG data URL of the icon at SIZE px, drawn by the browser, which
      // decodes what native code can't (SVG, ICO). Aspect ratio kept, centred
      // on transparent. Falls back to the raw bytes if drawing fails.
      var SIZE = 256;
      function rasterise(blob) {
        return new Promise(function (resolve) {
          var url = URL.createObjectURL(blob);
          var img = new Image();
          img.onload = function () {
            URL.revokeObjectURL(url);
            try {
              var w = img.naturalWidth || SIZE, h = img.naturalHeight || SIZE;
              var s = SIZE / Math.max(w, h);
              var dw = Math.round(w * s), dh = Math.round(h * s);
              var c = document.createElement('canvas');
              c.width = SIZE; c.height = SIZE;
              c.getContext('2d').drawImage(img, (SIZE - dw) / 2, (SIZE - dh) / 2, dw, dh);
              resolve(c.toDataURL('image/png'));
            } catch (e) { rawDataUrl(blob, resolve); }
          };
          img.onerror = function () { URL.revokeObjectURL(url); rawDataUrl(blob, resolve); };
          img.src = url;
        });
      }
      function rawDataUrl(blob, resolve) {
        var reader = new FileReader();
        reader.onloadend = function () { resolve(reader.result); };
        reader.onerror = function () { resolve(''); };
        reader.readAsDataURL(blob);
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
          done(best.rank, await rasterise(blob));
        } catch (e) { done(0); }
      }
      run();
    })();
    """
}
