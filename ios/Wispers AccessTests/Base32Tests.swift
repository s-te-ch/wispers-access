import Foundation
import Testing

@testable import Wispers_Access

struct Base32Tests {

    /// RFC 4648 §10 test vectors, both directions.
    @Test func rfc4648Vectors() {
        let cases: [(plain: String, encoded: String)] = [
            ("", ""),
            ("f", "MY"),
            ("fo", "MZXQ"),
            ("foo", "MZXW6"),
            ("foob", "MZXW6YQ"),
            ("fooba", "MZXW6YTB"),
            ("foobar", "MZXW6YTBOI"),
        ]
        for (plain, encoded) in cases {
            #expect(Base32.encode(Data(plain.utf8)) == encoded)
            #expect(Base32.decode(encoded) == Data(plain.utf8))
        }
    }

    @Test func decodeIsCaseInsensitive() {
        #expect(Base32.decode("mzxw6ytboi") == Data("foobar".utf8))
        #expect(Base32.decode("MzXw6YtBoI") == Data("foobar".utf8))
    }

    @Test func roundTripsAllLengths() {
        for n in 0...32 {
            let data = Data((0..<n).map { UInt8($0) })
            #expect(Base32.decode(Base32.encode(data)) == data, "round-trip failed at length \(n)")
        }
    }

    @Test func rejectsInvalidCharacters() {
        #expect(Base32.decode("MZXW0YTB") == nil)  // '0' is not in the alphabet
        #expect(Base32.decode("padded=") == nil)   // '=' padding is not accepted
    }

    @Test func rejectsNonCanonicalEncodings() {
        #expect(Base32.decode("A") == nil)   // 5 leftover bits — no valid length produces this
        #expect(Base32.decode("MZ") == nil)  // decodes 'f' but leaves non-zero trailing bits
    }
}
