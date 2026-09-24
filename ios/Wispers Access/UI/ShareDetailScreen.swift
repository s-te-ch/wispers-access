import SwiftUI
import WispersAccessSdk

/// Details for one share: online status, avatar, name, last-connected / joined
/// info, its apps to open, and Remove. Reads the live share from the manager by
/// id, so a change or removal reflects immediately.
struct ShareDetailScreen: View {
    let shareID: ShareId
    @Environment(ShareManager.self) private var manager
    @Environment(ShareIconStore.self) private var icons
    @Environment(\.dismiss) private var dismiss
    @State private var confirmingRemoval = false

    var body: some View {
        ZStack {
            AccessColor.background.ignoresSafeArea()
            if let share = manager.share(shareID) {
                content(share)
            } else {
                // Removed while open — leave the screen.
                Color.clear.onAppear { dismiss() }
            }
        }
        .navigationTitle("Shared with you")
        .navigationBarTitleDisplayMode(.inline)
        .task {
            while !Task.isCancelled {
                if let share = manager.share(shareID) {
                    await manager.status.refresh([share], using: manager.client, activity: manager.activity)
                }
                try? await Task.sleep(for: .seconds(30))
            }
        }
    }

    private func content(_ share: Share) -> some View {
        VStack(alignment: .leading, spacing: 16) {
            StatusRow(
                availability: share.state.availability
                    ?? manager.status.availability(for: shareID))
            ShareAvatar(nickname: name(share), iconPNG: icons.iconData(forAnyAppOf: share), size: 64)
            Text(name(share))
                .font(.system(.largeTitle, design: .serif).weight(.bold))
                .foregroundStyle(AccessColor.onSurface)
            HStack(spacing: 8) {
                InfoCard(label: "LAST CONNECTED", value: date(manager.activity.lastConnected(shareID)))
                InfoCard(label: "JOINED", value: date(share.joinedAt))
            }
            if share.state != .live {
                TerminalShareExplanation(state: share.state)
                removeButton
            } else {
                VStack(spacing: 8) {
                    if share.apps.isEmpty {
                        Text("No apps shared yet. They appear here once the host adds some.")
                            .font(.footnote).foregroundStyle(AccessColor.onSurfaceVariant)
                    }
                    ForEach(share.apps, id: \.id) { app in
                        NavigationLink(value: ShareRoute.browse(BrowseKey(shareID: shareID, appID: app.id))) {
                            Text("Open \(app.name) ↗").accessFilledButton()
                        }
                    }
                    removeButton
                }
            }
            Spacer()
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .confirmationDialog("Remove \(name(share))?", isPresented: $confirmingRemoval) {
            Button("Remove", role: .destructive) {
                manager.delete(shareID)
                dismiss()
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("This device's access will be removed on the host. You'll need a new invitation code to rejoin.")
        }
    }

    private var removeButton: some View {
        Button {
            confirmingRemoval = true
        } label: {
            Text("Remove from this device").accessOutlinedButton(tint: AccessColor.destructive)
        }
    }

    private func name(_ share: Share) -> String {
        share.name.isEmpty ? "Untitled share" : share.name
    }

    private func date(_ date: Date?) -> String {
        guard let date else { return "—" }
        return Self.formatter.string(from: date)
    }

    private static let formatter: DateFormatter = {
        let formatter = DateFormatter()
        formatter.dateFormat = "yyyy-MM-dd"
        return formatter
    }()
}

private struct StatusRow: View {
    let availability: Availability

    var body: some View {
        HStack(spacing: 8) {
            StatusDot(availability: availability, size: 10)
            Text(label)
                .font(.caption.weight(.medium)).tracking(0.5)
                .foregroundStyle(AccessColor.onSurfaceVariant)
        }
    }

    private var label: String {
        switch availability {
        case .online: return "ONLINE"
        case .offline: return "OFFLINE"
        case .unknown: return "UNKNOWN"
        case .checking: return "CHECKING…"
        case .removed, .revoked: return "NO LONGER AVAILABLE"
        }
    }
}

private struct InfoCard: View {
    let label: String
    let value: String

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(label)
                .font(.caption2)
                .foregroundStyle(AccessColor.onSurfaceVariant)
            Text(value)
                .font(.body)
                .foregroundStyle(AccessColor.onSurface)
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(AccessColor.infoCard, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
    }
}
