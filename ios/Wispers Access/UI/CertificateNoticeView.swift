import SwiftUI
import WispersAccessSdk

/// What a forward-proxy app shows instead of its page once the page was
/// refused for its certificate. If the share has a CA certificate, it offers
/// it as a configuration profile, and tries the page again when the user comes
/// back from installing it. Otherwise it says the share's host has to fix the
/// certificate.
struct CertificateNoticeView: View {
    let share: Share
    let onRetry: () -> Void

    @Environment(\.scenePhase) private var scenePhase
    @State private var server: ProfileServer?
    @State private var failure: String?

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                Text("This app's certificate isn't trusted").font(.title2.bold())
                let certificates = ConfigurationProfile.certificates(of: share)
                if let ca = certificates.first(where: { $0.description.kind == .ca }) {
                    offer(ca.description)
                } else if let other = certificates.first {
                    Text(notInstallable(other.description))
                } else {
                    Text(
                        "This device doesn't trust the certificate of the app's servers, and \(shareName) has none to install. Whoever runs the share has to fix this."
                    )
                }
                if let failure {
                    Text(failure).foregroundStyle(.red)
                }
                Button("Try again", action: onRetry).buttonStyle(.bordered)
            }
            .padding(24)
            .frame(maxWidth: 560, alignment: .leading)
            .frame(maxWidth: .infinity)
        }
        .background(Color(.systemBackground))
        // Back from Safari and Settings.
        .onChange(of: scenePhase) { _, phase in
            if phase == .active, server != nil { onRetry() }
        }
        .onDisappear {
            server?.stop()
            server = nil
        }
    }

    private func offer(_ ca: CertificateDescription) -> some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("\(shareName) comes with the certificate of “\(ca.subject)”, which this device needs to trust the app's servers.")
            Text("Once it's installed and trusted, whoever holds it could vouch for any website on this device. Only install it if you trust \(shareName).")
                .font(.callout)
            Text(
                "Safari asks whether to allow a configuration profile: tap Allow. Then, in Settings, tap “Profile Downloaded” and install it. Last, under General › About › Certificate Trust Settings, turn on full trust for “\(ca.subject)”. Then come back here."
            )
            .font(.callout)
            Button("Download Profile") { Task { await handOver() } }
                .buttonStyle(.borderedProminent)
        }
    }

    /// Serves the profile on loopback and opens it in Safari.
    private func handOver() async {
        guard let profile = ConfigurationProfile.data(for: share) else { return }
        server?.stop()
        let server = ProfileServer(profile: profile)
        do {
            let url = try await server.start()
            self.server = server
            failure = nil
            await UIApplication.shared.open(url)
        } catch {
            failure = "Couldn't hand the profile to Safari: \(error.localizedDescription)"
        }
    }

    /// Why a share's certificate that isn't a CA's can't help: iOS installs
    /// it, but offers full trust only for CAs.
    private func notInstallable(_ certificate: CertificateDescription) -> String {
        let names = certificate.names.isEmpty ? certificate.subject : certificate.names.joined(separator: ", ")
        return switch certificate.kind {
        case .selfSignedServer:
            "\(shareName) comes with the certificate of \(names), which this device can't trust: it trusts only CA certificates. Whoever runs the share can make one."
        default:
            "\(shareName) comes with the certificate of \(names) instead of its CA's. Whoever runs the share has to add the CA's certificate."
        }
    }

    private var shareName: String { share.name.isEmpty ? "This share" : share.name }
}
