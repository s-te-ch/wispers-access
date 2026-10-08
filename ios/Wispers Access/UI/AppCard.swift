import SwiftUI
import WispersAccessSdk

/// One app's card: its icon (harvested while browsing, else a letter tile)
/// and name. The thing the user taps.
struct AppCard: View {
    let app: SharedApp
    let isLive: Bool
    let iconData: Data?
    /// Whether the tap opens a window of its own (iPad) rather than pushing a
    /// screen, which the trailing mark says.
    var opensWindow = false

    var body: some View {
        HStack(spacing: 16) {
            ShareAvatar(nickname: name, iconPNG: iconData, size: 48)
                .overlay(alignment: .topTrailing) {
                    if isLive { liveBadge }
                }
            Text(name)
                .font(.system(.title3, design: .serif).weight(.bold))
                .foregroundStyle(AccessColor.onSurface)
                .fixedSize(horizontal: false, vertical: true)
                .multilineTextAlignment(.leading)
            Spacer(minLength: 8)
            if opensWindow {
                openPill
            } else {
                Image(systemName: "chevron.right")
                    .font(.footnote.weight(.semibold))
                    .foregroundStyle(AccessColor.onSurfaceVariant)
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 16)
        .background(AccessColor.surface, in: RoundedRectangle(cornerRadius: 16, style: .continuous))
        .contentShape(Rectangle())
    }

    /// Subtle presence badge on the avatar: this app has a warm session open.
    private var liveBadge: some View {
        Circle()
            .fill(AccessColor.primaryDark)
            .frame(width: 13, height: 13)
            .overlay(Circle().stroke(AccessColor.surface, lineWidth: 2.5))
            .offset(x: 3, y: -3)
            .accessibilityLabel("Open")
    }

    /// The desktop app's "Open in browser ↗", for a window of the app's own.
    private var openPill: some View {
        HStack(spacing: 6) {
            Text("Open")
            Image(systemName: "arrow.up.right")
        }
        .font(.subheadline.weight(.semibold))
        .foregroundStyle(AccessColor.primaryDark)
        .padding(.horizontal, 16)
        .frame(height: 40)
        .background(AccessColor.primary, in: Capsule())
    }

    private var name: String {
        app.name.isEmpty ? app.id : app.name
    }
}
