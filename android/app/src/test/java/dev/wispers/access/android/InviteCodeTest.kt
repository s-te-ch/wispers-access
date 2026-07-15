package dev.wispers.access.android

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class InviteCodeTest {

    @Test
    fun parsesWaxCode() {
        assertEquals(
            InviteCode(registrationToken = "ab12cd", activationCode = "1-xyz789", backend = null),
            InviteCode.parse("wax_ab12cd_1-xyz789").getOrNull(),
        )
    }

    @Test
    fun toleratesPastedWhitespace() {
        assertEquals(
            InviteCode(registrationToken = "ab12cd", activationCode = "1-xyz789", backend = null),
            InviteCode.parse("  wax_ab12cd_1-xyz789\n").getOrNull(),
        )
    }

    @Test
    fun parsesWaxCodeWithBackend() {
        val url = "https://myhub.example.com"
        assertEquals(
            InviteCode(registrationToken = "ab12cd", activationCode = "1-xyz789", backend = url),
            InviteCode.parse("wax_ab12cd_1-xyz789_${wireBackend(url)}").getOrNull(),
        )
    }

    @Test
    fun rejectsMalformedCodes() {
        assertTrue(InviteCode.parse("ab12cd/1-xyz789").isFailure) // old test format
        assertTrue(InviteCode.parse("wax_ab12cd").isFailure) // missing activation code
        assertTrue(InviteCode.parse("wax__1-xyz789").isFailure) // empty token
        assertTrue(InviteCode.parse("wax_ab12cd_").isFailure) // empty activation code
        assertTrue(InviteCode.parse("https://example.com/not-an-invite").isFailure)
        assertTrue(InviteCode.parse("").isFailure)
    }

    @Test
    fun rejectsBadBackendField() {
        // A present-but-unusable backend fails the whole code shut (never falls
        // back to the managed hub) and says why.
        val nonHttps = wireBackend("http://evil.example.com")
        val err = InviteCode.parse("wax_ab12cd_1-xyz789_$nonHttps").exceptionOrNull()
        assertTrue(err?.message?.contains("https") == true)
        assertTrue(InviteCode.parse("wax_ab12cd_1-xyz789_!!notbase32").isFailure)
    }

    /**
     * A backend URL as it appears on the wire: base32 of the UTF-8 bytes,
     * lowercased — the exact shape `waserver`'s `compose_wax_code` emits.
     */
    private fun wireBackend(url: String): String =
        Base32.encode(url.toByteArray(Charsets.UTF_8)).lowercase()
}
