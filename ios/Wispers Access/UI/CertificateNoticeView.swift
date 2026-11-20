import SwiftUI
import WispersAccessSdk

/// What a forward-proxy app shows instead of its page once the page was
/// refused for its certificate. If the share has a CA certificate, it walks the
/// user through installing it as a configuration profile, and tries the page
/// again when the user comes back from Safari and Settings. Otherwise it says
/// the share's host has to fix the certificate.
struct CertificateNoticeView: View {
    let share: Share
    /// The session whose page was refused. It keeps the profile server, as
    /// every retry replaces this view.
    let session: BrowseSession

    @Environment(\.scenePhase) private var scenePhase
    @State private var failure: String?

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 20) {
                let certificates = ConfigurationProfile.certificates(of: share)
                if let ca = certificates.first(where: { $0.description.kind == .ca }) {
                    steps(ca.description)
                } else {
                    Text("This app's certificate isn't trusted").font(.title2.bold())
                    if let other = certificates.first {
                        Text(notInstallable(other.description))
                    } else {
                        Text(
                            "This device doesn't trust the certificate of the app's servers, and the share has none to install. Whoever runs the share has to fix this."
                        )
                    }
                    Button("Try Again") { session.start() }.buttonStyle(.bordered)
                }
            }
            .padding(24)
            .frame(maxWidth: 560, alignment: .leading)
            .frame(maxWidth: .infinity)
        }
        .background(Color(.systemBackground))
        // Back from Safari and Settings.
        .onChange(of: scenePhase) { _, phase in
            if phase == .active, session.profileServer != nil { session.start() }
        }
    }

    /// The installation as numbered steps, as it spans the app, Safari and
    /// Settings. Safari's page repeats the steps after the download, since the
    /// user can't see this view from there.
    @ViewBuilder
    private func steps(_ ca: CertificateDescription) -> some View {
        let downloaded = session.profileServer != nil
        Text(ownerName.map { "Install \($0)'s certificate" } ?? "Install the share's certificate")
            .font(.title2.bold())
        Text("This app's servers use a certificate from “\(ca.subject)”. Your \(device) has to trust it before the app can open.")
        VStack(alignment: .leading, spacing: 18) {
            StepRow(number: 1, title: "Download the profile", done: downloaded) {
                Text("Safari opens and asks whether to allow it. Tap Allow.")
                if downloaded {
                    Button("Download Again") { Task { await handOver() } }
                        .buttonStyle(.bordered)
                } else {
                    Button("Download Profile") { Task { await handOver() } }
                        .buttonStyle(.borderedProminent)
                }
            }
            StepRow(number: 2, title: "Install it") {
                Text("Open Settings › General › VPN & Device Management. Tap “\(profileName)”, then Install.")
            }
            StepRow(number: 3, title: "Turn on trust") {
                Text("Go back to General › About. At the very bottom, tap Certificate Trust Settings and turn on “\(ca.subject)”.")
            }
            StepRow(number: 4, title: "Come back here") {
                Text("The app opens by itself once your \(device) trusts the certificate.")
            }
        }
        if let failure {
            Text(failure).foregroundStyle(.red)
        }
        Label {
            Text("Only do this if you trust \(ownerName ?? "this share"). Once installed and trusted, the certificate lets its owner make any website look secure.")
        } icon: {
            Image(systemName: "exclamationmark.shield")
        }
        .font(.footnote)
        .foregroundStyle(.secondary)
        Button("Check Again") { session.start() }
            .font(.footnote)
    }

    /// Serves the profile and its instructions page on loopback and opens the
    /// page in Safari, which starts the download from there.
    private func handOver() async {
        guard let profile = ConfigurationProfile.data(for: share),
            let ca = ConfigurationProfile.certificates(of: share).first(where: { $0.description.kind == .ca })
        else { return }
        session.profileServer?.stop()
        session.profileServer = nil
        let page = ConfigurationProfile.instructionsPage(
            profileName: profileName, caSubject: ca.description.subject, device: device)
        let server = ProfileServer(profile: profile, page: page)
        do {
            let url = try await server.start()
            session.profileServer = server
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
            "\(shareName) comes with a certificate for \(names), which this device can't trust, as it fully trusts only CA certificates. Whoever runs the share can make one."
        default:
            "\(shareName) comes with a certificate for \(names) instead of its CA's. Whoever runs the share has to add the CA's certificate."
        }
    }

    private var shareName: String { ownerName.map { "“\($0)”" } ?? "This share" }

    /// The share's name, whose host owns the certificate. `nil` if it has none.
    private var ownerName: String? { share.name.isEmpty ? nil : share.name }

    /// The profile's name in Settings.
    private var profileName: String { ConfigurationProfile.displayName(for: share) }

    /// "iPhone" or "iPad", for the steps.
    private var device: String { UIDevice.current.model }
}

/// One numbered step of `CertificateNoticeView`, with a checkmark once done.
private struct StepRow<Content: View>: View {
    let number: Int
    let title: String
    var done = false
    @ViewBuilder let content: Content

    var body: some View {
        HStack(alignment: .top, spacing: 14) {
            ZStack {
                Circle().fill(done ? AccessColor.online : AccessColor.primary)
                if done {
                    Image(systemName: "checkmark").font(.footnote.bold()).foregroundStyle(.white)
                } else {
                    Text("\(number)").font(.subheadline.bold()).foregroundStyle(AccessColor.primaryDark)
                }
            }
            .frame(width: 28, height: 28)
            .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 6) {
                Text(title).font(.headline)
                    .accessibilityLabel(done ? "Step \(number), done. \(title)" : "Step \(number). \(title)")
                content.font(.callout)
            }
        }
    }
}
