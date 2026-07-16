import Foundation

/// A parsed `wax_<token>_<activation_code>[_<backend>]` invite code, as produced
/// by `waserver invite`. Mirrors waclient's and the Android app's parser.
struct InviteCode: Equatable {
    let registrationToken: String
    let activationCode: String
    let backend: String?

    private nonisolated static let prefix = "wax_"

    /// Parses `raw` into an `InviteCode`. Tolerates surrounding whitespace (codes
    /// arrive via paste and QR). Throws an `InviteCodeError` carrying a readable
    /// message rather than returning nil, so a bad *backend* field explains
    /// itself instead of masquerading as a malformed code.
    nonisolated static func parse(_ raw: String) throws -> InviteCode {
        let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard trimmed.hasPrefix(prefix) else { throw InviteCodeError.malformed }
        let rest = trimmed.dropFirst(prefix.count)
        // At most three fields; any further underscores stay in the backend field
        // (base32 has none, the token is hex and the activation code base36, so
        // this splits cleanly). Empty fields are kept so they can be rejected.
        let parts = rest.split(separator: "_", maxSplits: 2, omittingEmptySubsequences: false)
        let registrationToken = parts.count > 0 ? String(parts[0]) : ""
        let activationCode = parts.count > 1 ? String(parts[1]) : ""
        guard !registrationToken.isEmpty, !activationCode.isEmpty else {
            throw InviteCodeError.malformed
        }
        let backend = parts.count > 2 ? try decodeBackend(String(parts[2])) : nil
        return InviteCode(
            registrationToken: registrationToken,
            activationCode: activationCode,
            backend: backend
        )
    }

    /// Decodes the invite's base32 backend field back to its URL, throwing when
    /// the field is present but unusable. Accepts only `https://` URLs.
    private nonisolated static func decodeBackend(_ encoded: String) throws -> String {
        guard let bytes = Base32.decode(encoded) else {
            throw InviteCodeError.badBackend("invite's backend field is not valid base32")
        }
        guard let url = String(data: bytes, encoding: .utf8) else {
            throw InviteCodeError.badBackend("invite's backend URL is not valid UTF-8")
        }
        guard url.hasPrefix("https://") else {
            throw InviteCodeError.badBackend("invite's backend URL must be https:// (got \"\(url)\")")
        }
        return url
    }
}

/// Why an invite code couldn't be parsed. `malformed` covers the fixed fields;
/// `badBackend` carries the specific reason the optional backend field failed,
/// so a self-hosted invite that's subtly wrong doesn't look like a typo.
enum InviteCodeError: LocalizedError, Equatable {
    case malformed
    case badBackend(String)

    var errorDescription: String? {
        switch self {
        case .malformed:
            return "Not a Wispers invite code (expected wax_…)."
        case .badBackend(let message):
            return message
        }
    }
}
