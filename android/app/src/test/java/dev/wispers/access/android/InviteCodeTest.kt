package dev.wispers.access.android

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class InviteCodeTest {

    @Test
    fun parsesWaxCode() {
        assertEquals(
            InviteCode(registrationToken = "ab12cd", activationCode = "1-xyz789"),
            InviteCode.parse("wax_ab12cd_1-xyz789"),
        )
    }

    @Test
    fun toleratesPastedWhitespace() {
        assertEquals(
            InviteCode(registrationToken = "ab12cd", activationCode = "1-xyz789"),
            InviteCode.parse("  wax_ab12cd_1-xyz789\n"),
        )
    }

    @Test
    fun rejectsMalformedCodes() {
        assertNull(InviteCode.parse("ab12cd/1-xyz789")) // old test format
        assertNull(InviteCode.parse("wax_ab12cd")) // missing activation code
        assertNull(InviteCode.parse("wax__1-xyz789")) // empty token
        assertNull(InviteCode.parse("wax_ab12cd_")) // empty activation code
        assertNull(InviteCode.parse("https://example.com/not-an-invite"))
        assertNull(InviteCode.parse(""))
    }
}
