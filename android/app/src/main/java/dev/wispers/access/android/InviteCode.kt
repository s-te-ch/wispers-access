package dev.wispers.access.android

/**
 * A parsed `wax_<token>_<activation_code>` invite code, as produced by
 * `waserver invite`. Mirrors waclient's parser.
 */
data class InviteCode(
    val registrationToken: String,
    val activationCode: String,
) {
    companion object {
        /**
         * Parses [raw] into an [InviteCode], or returns null if malformed.
         * Tolerates surrounding whitespace (codes arrive via paste and QR).
         * The token is lowercase hex, so the first `_` after the prefix
         * unambiguously ends it; the activation code keeps its native
         * `<node>-<secret>` form.
         */
        fun parse(raw: String): InviteCode? {
            val rest = raw.trim().removePrefix(PREFIX)
            if (rest.length == raw.trim().length) return null // prefix missing
            val sep = rest.indexOf('_')
            if (sep <= 0 || sep == rest.length - 1) return null
            return InviteCode(
                registrationToken = rest.substring(0, sep),
                activationCode = rest.substring(sep + 1),
            )
        }

        private const val PREFIX = "wax_"
    }
}
