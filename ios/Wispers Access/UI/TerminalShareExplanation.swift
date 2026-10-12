import SwiftUI
import WispersAccessSdk

/// Explains why a share can't be used - what happened, and what remains to do,
/// e.g. update the app, or remove the share. Mirrors the Android
/// `TerminalShareExplanation`, so the story reads the same on both platforms.
struct TerminalShareExplanation: View {
    let state: ShareState

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(title)
                .font(.headline)
                .foregroundStyle(AccessColor.onSurface)
            Text(explanation)
                .font(.subheadline)
                .foregroundStyle(AccessColor.onSurfaceVariant)
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(AccessColor.infoCard, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
    }

    private var title: String {
        switch state {
        case .outdated: "This app is too old for this share"
        case .removed, .revoked, .live: "This share is no longer available"
        }
    }

    private var explanation: String {
        let what: String
        switch state {
        case .outdated:
            return "The share's host runs a newer version of Wispers Access than this app understands. Update the app to keep using the share."
        case .removed:
            what = "The share was removed by its host and can't be reached anymore."
        case .revoked:
            what = "This device's access was revoked by the share's host."
        case .live:
            what = "The share is available."
        }
        return what + " You can remove it from this device; joining again needs a new invitation code."
    }
}
