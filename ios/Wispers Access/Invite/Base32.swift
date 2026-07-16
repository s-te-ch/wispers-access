import Foundation

/// RFC 4648 base32 codec, no padding — the encoding waserver uses for the
/// backend URL it embeds in an invite code. Foundation ships a base64 codec
/// (`Data(base64Encoded:)`) but no base32, so this is a direct port of the
/// Android `Base32` object.
///
/// `encode` emits the canonical uppercase alphabet; `decode` accepts either
/// case (invite codes lowercase it to sit alongside the hex token and base36
/// activation code).
enum Base32 {
    private nonisolated static let alphabet = Array("ABCDEFGHIJKLMNOPQRSTUVWXYZ234567".utf8)

    /// Encodes `data` to unpadded base32 (uppercase). Empty in, empty out.
    nonisolated static func encode(_ data: Data) -> String {
        var out = [UInt8]()
        out.reserveCapacity((data.count * 8 + 4) / 5)  // ceil(8N/5)
        var buffer = 0
        var bits = 0
        for byte in data {
            buffer = (buffer << 8) | Int(byte)
            bits += 8
            while bits >= 5 {
                bits -= 5
                out.append(alphabet[(buffer >> bits) & 0x1F])
            }
        }
        if bits > 0 {
            // Pad the final partial group's low bits with zeros.
            out.append(alphabet[(buffer << (5 - bits)) & 0x1F])
        }
        return String(decoding: out, as: UTF8.self)
    }

    /// Decodes unpadded base32 (case-insensitive) to bytes, or returns nil on an
    /// invalid character, an invalid length, or non-canonical (non-zero) trailing
    /// bits. An empty string decodes to empty data.
    nonisolated static func decode(_ input: String) -> Data? {
        var buffer: UInt64 = 0
        var bits = 0
        var out = Data()
        out.reserveCapacity(input.count * 5 / 8)
        for ascii in input.uppercased().utf8 {
            guard let value = alphabet.firstIndex(of: ascii) else { return nil }
            buffer = (buffer << 5) | UInt64(value)
            bits += 5
            if bits >= 8 {
                bits -= 8
                out.append(UInt8((buffer >> UInt64(bits)) & 0xFF))
            }
        }
        // A full leftover group (>=5 bits) means an invalid length; any leftover
        // bits must be zero padding to be canonical.
        if bits >= 5 || (buffer & ((1 << UInt64(bits)) - 1)) != 0 { return nil }
        return out
    }
}
