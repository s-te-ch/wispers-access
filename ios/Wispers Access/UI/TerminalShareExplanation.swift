import SwiftUI

/// Explains a terminal share: what happened and that only removal remains.
/// Mirrors the Android `TerminalShareExplanation`, so the story reads the same
/// on both platforms.
struct TerminalShareExplanation: View {
    let state: TerminalShareState

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("This share is no longer available")
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

    private var explanation: String {
        let what: String
        switch state {
        case .removed:
            what = "The share was removed by its owner and can't be reached anymore."
        case .revoked:
            what = "This device's access to the share was revoked by its owner."
        }
        return what + " You can remove it from this device; joining again needs a new invitation code."
    }
}
