package dev.wispers.access.android

/**
 * A parsed `wax_<token>_<activation_code>[_<backend>]` invite code, as produced
 * by `waserver invite`. Mirrors waclient's parser.
 */
data class InviteCode(
    val registrationToken: String,
    val activationCode: String,
    val backend: String? = null,
) {
    companion object {
        /**
         * Parses [raw] into an [InviteCode]. Tolerates surrounding whitespace
         * (codes arrive via paste and QR). Returns a failure carrying a readable
         * message rather than a bare null, so a bad *backend* field explains
         * itself instead of masquerading as a malformed code.
         */
        fun parse(raw: String): Result<InviteCode> {
            val trimmed = raw.trim()
            val rest = trimmed.removePrefix(PREFIX)
            if (rest.length == trimmed.length) return malformed()
            val parts = rest.split('_', limit = 3)
            val registrationToken = parts.getOrElse(0) { "" }
            val activationCode = parts.getOrElse(1) { "" }
            if (registrationToken.isEmpty() || activationCode.isEmpty()) return malformed()
            val backend = if (parts.size > 2) {
                decodeBackend(parts[2]).getOrElse { return Result.failure(it) }
            } else {
                null
            }
            return Result.success(InviteCode(registrationToken, activationCode, backend))
        }

        /**
         * Decodes the invite's base32 backend field back to its URL, failing
         * when the field is present but unusable. Accepts only `https://` URLs.
         */
        private fun decodeBackend(encoded: String): Result<String> {
            val bytes = Base32.decode(encoded)
                ?: return failure("invite's backend field is not valid base32")
            val url = bytes.toString(Charsets.UTF_8)
            if (!url.startsWith("https://")) {
                return failure("invite's backend URL must be https:// (got \"$url\")")
            }
            return Result.success(url)
        }

        private fun malformed(): Result<InviteCode> =
            failure("Not a Wispers invite code (expected wax_…).")

        private fun failure(message: String): Result<Nothing> =
            Result.failure(IllegalArgumentException(message))

        private const val PREFIX = "wax_"
    }
}
