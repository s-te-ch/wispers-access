package dev.wispers.access.android.proxy

import dev.wispers.access.android.storage.ShareId

/** A single HTTP header. Mutable so rewriters can edit in place. */
data class Header(var name: String, var value: String)

/**
 * Pluggable header transforms applied at the proxy boundary, both directions.
 *
 * Operates in place on an ordered, duplicate-allowing list (Set-Cookie can appear
 * multiple times in a response). The proxy core strips hop-by-hop headers and
 * manages framing (Content-Length / Transfer-Encoding) around the rewriter call,
 * so a rewriter can't accidentally break framing.
 */
interface HeaderRewriter {
    fun rewriteRequest(
        method: String,
        target: String,
        headers: MutableList<Header>,
        shareId: ShareId,
    )

    fun rewriteResponse(
        status: Int,
        headers: MutableList<Header>,
        shareId: ShareId,
    )
}

class DefaultHeaderRewriter : HeaderRewriter {
    override fun rewriteRequest(
        method: String,
        target: String,
        headers: MutableList<Header>,
        shareId: ShareId,
    ) {
        // No-op for now. Hooks for Host rewriting, Origin/Referer scrubbing land here.
    }

    override fun rewriteResponse(
        status: Int,
        headers: MutableList<Header>,
        shareId: ShareId,
    ) {
        // Strip Set-Cookie Domain= attribute so cookies bind to <shareId>.localhost
        // (the host the WebView talks to), not whatever the upstream thought it was.
        for (h in headers) {
            if (h.name.equals("set-cookie", ignoreCase = true)) {
                h.value = stripDomainAttribute(h.value)
            }
        }
    }

    private fun stripDomainAttribute(setCookie: String): String =
        setCookie.split(";")
            .filterNot { it.trim().startsWith("domain=", ignoreCase = true) }
            .joinToString(";")
}
