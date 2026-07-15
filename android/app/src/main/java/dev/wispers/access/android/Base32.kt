package dev.wispers.access.android

/**
 * RFC 4648 base32 codec, no padding, the encoding waserver uses for the
 * backend URL it embeds in an invite code. Kotlin/Java ship a base64 codec
 * (`java.util.Base64`) but no base32, so this is hand-rolled.
 *
 * [encode] emits the canonical uppercase alphabet; [decode] accepts either case
 * (invite codes lowercase it to sit alongside the hex token and base36
 * activation code).
 */
object Base32 {
    private const val ALPHABET = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567"

    /** Encodes [data] to unpadded base32 (uppercase). Empty in, empty out. */
    fun encode(data: ByteArray): String {
        val out = StringBuilder((data.size * 8 + 4) / 5)  // ceil(8/5)
        var buffer = 0
        var bits = 0
        for (b in data) {
            buffer = (buffer shl 8) or (b.toInt() and 0xFF)
            bits += 8
            while (bits >= 5) {
                bits -= 5
                out.append(ALPHABET[(buffer shr bits) and 0x1F])
            }
        }
        if (bits > 0) {
            // Pad the final partial group's low bits with zeros.
            out.append(ALPHABET[(buffer shl (5 - bits)) and 0x1F])
        }
        return out.toString()
    }

    /**
     * Decodes unpadded base32 (case-insensitive) to bytes, or returns null on an
     * invalid character, an invalid length, or non-canonical (non-zero) trailing
     * bits. An empty string decodes to an empty array.
     */
    fun decode(input: String): ByteArray? {
        var buffer = 0L
        var bits = 0
        val out = ArrayList<Byte>(input.length * 5 / 8)
        for (ch in input.uppercase()) {
            val v = ALPHABET.indexOf(ch)
            if (v < 0) return null
            buffer = (buffer shl 5) or v.toLong()
            bits += 5
            if (bits >= 8) {
                bits -= 8
                out.add(((buffer shr bits) and 0xFF).toByte())
            }
        }
        // A full leftover group (>=5 bits) means an invalid length; any leftover
        // bits must be zero padding to be canonical.
        if (bits >= 5 || (buffer and ((1L shl bits) - 1)) != 0L) return null
        return out.toByteArray()
    }
}
