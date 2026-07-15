package dev.wispers.access.android

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class Base32Test {

    // RFC 4648 §10 test vectors, with the `=` padding stripped (we use the
    // no-pad variant). These are the ground truth pinning both directions.
    private val vectors = listOf(
        "" to "",
        "f" to "MY",
        "fo" to "MZXQ",
        "foo" to "MZXW6",
        "foob" to "MZXW6YQ",
        "fooba" to "MZXW6YTB",
        "foobar" to "MZXW6YTBOI",
    )

    @Test
    fun encodesRfc4648Vectors() {
        for ((plain, encoded) in vectors) {
            assertEquals(encoded, Base32.encode(plain.toByteArray(Charsets.UTF_8)))
        }
    }

    @Test
    fun decodesRfc4648Vectors() {
        for ((plain, encoded) in vectors) {
            assertArrayEquals(plain.toByteArray(Charsets.UTF_8), Base32.decode(encoded))
        }
    }

    @Test
    fun decodeIsCaseInsensitive() {
        assertArrayEquals("foobar".toByteArray(), Base32.decode("mzxw6ytboi"))
        assertArrayEquals("foobar".toByteArray(), Base32.decode("MzXw6YtBoI"))
    }

    @Test
    fun roundTripsArbitraryBytes() {
        for (len in 0..32) {
            val data = ByteArray(len) { (it * 37 + 11).toByte() }
            assertArrayEquals(data, Base32.decode(Base32.encode(data)))
        }
    }

    @Test
    fun rejectsInvalidCharacters() {
        assertNull(Base32.decode("MY0")) // 0 is not in the base32 alphabet
        assertNull(Base32.decode("MY=")) // padding is not accepted in no-pad
        assertNull(Base32.decode("!!"))
    }

    @Test
    fun rejectsNonCanonicalEncodings() {
        assertNull(Base32.decode("A")) // 5 leftover bits: no valid length ends here
        assertNull(Base32.decode("MZ")) // decodes 'f', but its trailing bits aren't zero
    }
}
