import SwiftUI

/// Details for one share: online status, avatar, name, join / last-connected
/// info, and Open / Remove actions. Mirrors the Android `ShareDetailScreen`
/// (minus "Add to homescreen", which has no iOS equivalent). Reads the live
/// share from the store by id, so a rename or removal reflects immediately.
struct ShareDetailScreen: View {
    let shareID: ShareID
    @Environment(ShareStore.self) private var store
    @Environment(ShareManager.self) private var manager
    @Environment(ShareIconStore.self) private var icons
    @Environment(\.dismiss) private var dismiss
    @State private var confirmingRemoval = false

    var body: some View {
        ZStack {
            AccessColor.background.ignoresSafeArea()
            if let share = store.metadata(for: shareID) {
                content(share)
            } else {
                // Removed while open — leave the screen.
                Color.clear.onAppear { dismiss() }
            }
        }
        .navigationTitle("Shared app")
        .navigationBarTitleDisplayMode(.inline)
        .task {
            while !Task.isCancelled {
                await manager.status.refresh([shareID], using: manager.sessions, store: store)
                try? await Task.sleep(for: .seconds(30))
            }
        }
    }

    private func content(_ share: ShareMetadata) -> some View {
        VStack(alignment: .leading, spacing: 16) {
            StatusRow(
                availability: share.terminalState?.availability
                    ?? manager.status.availability(for: shareID))
            ShareAvatar(nickname: name(share), iconPNG: icons.iconData(for: shareID), size: 64)
            Text(name(share))
                .font(.system(.largeTitle, design: .serif).weight(.bold))
                .foregroundStyle(AccessColor.onSurface)
            HStack(spacing: 8) {
                InfoCard(label: "LAST CONNECTED", value: date(share.lastConnectedAt))
                InfoCard(label: "JOINED", value: date(share.createdAt))
            }
            if let terminal = share.terminalState {
                TerminalShareExplanation(state: terminal)
                Button {
                    confirmingRemoval = true
                } label: {
                    Text("Remove from this device")
                        .accessOutlinedButton(tint: AccessColor.destructive)
                }
            } else {
                VStack(spacing: 8) {
                    NavigationLink(value: ShareRoute.browse(shareID)) {
                        Text("Open app ↗").accessFilledButton()
                    }
                    Button {
                        confirmingRemoval = true
                    } label: {
                        Text("Remove from this device").accessOutlinedButton(tint: AccessColor.destructive)
                    }
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
            Text("This device's access will be removed on the server. You'll need a new invitation code to rejoin.")
        }
    }

    private func name(_ share: ShareMetadata) -> String {
        share.nickname.isEmpty ? "Untitled app" : share.nickname
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
