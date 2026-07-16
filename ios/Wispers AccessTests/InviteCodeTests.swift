import Foundation
import Testing

@testable import Wispers_Access

struct InviteCodeTests {

    /// Encodes a backend URL exactly as `waserver` puts it on the wire: base32,
    /// lowercased to sit alongside the hex token and base36 activation code.
    private func wireBackend(_ url: String) -> String {
        Base32.encode(Data(url.utf8)).lowercased()
    }

    @Test func parsesManagedCode() throws {
        let code = try InviteCode.parse("wax_deadbeef01_1-abc123xyz")
        #expect(code.registrationToken == "deadbeef01")
        #expect(code.activationCode == "1-abc123xyz")
        #expect(code.backend == nil)
    }

    @Test func trimsSurroundingWhitespace() throws {
        let code = try InviteCode.parse("  wax_tok_1-secret\n")
        #expect(code.registrationToken == "tok")
        #expect(code.activationCode == "1-secret")
        #expect(code.backend == nil)
    }

    @Test func parsesSelfHostedBackend() throws {
        let url = "https://hub.example.ch"
        let code = try InviteCode.parse("wax_tok_1-secret_" + wireBackend(url))
        #expect(code.registrationToken == "tok")
        #expect(code.activationCode == "1-secret")
        #expect(code.backend == url)
    }

    @Test func rejectsMissingPrefix() {
        expectMalformed("nope_tok_act")
        expectMalformed("")
    }

    @Test func rejectsMissingFields() {
        expectMalformed("wax_")
        expectMalformed("wax_tokenonly")  // no activation code
        expectMalformed("wax__1-secret")  // empty token
    }

    @Test func rejectsNonBase32Backend() {
        expectBadBackend("wax_tok_1-secret_not!base32")
    }

    @Test func rejectsNonHttpsBackend() {
        expectBadBackend("wax_tok_1-secret_" + wireBackend("http://insecure.example"))
    }

    // MARK: - Helpers

    private func expectMalformed(_ raw: String) {
        do {
            _ = try InviteCode.parse(raw)
            Issue.record("expected \"\(raw)\" to be rejected as malformed")
        } catch let error as InviteCodeError {
            #expect(error == .malformed)
        } catch {
            Issue.record("unexpected error \(error)")
        }
    }

    private func expectBadBackend(_ raw: String) {
        do {
            _ = try InviteCode.parse(raw)
            Issue.record("expected \"\(raw)\" to be rejected for its backend field")
        } catch let error as InviteCodeError {
            guard case .badBackend = error else {
                Issue.record("expected badBackend, got \(error)")
                return
            }
        } catch {
            Issue.record("unexpected error \(error)")
        }
    }
}
