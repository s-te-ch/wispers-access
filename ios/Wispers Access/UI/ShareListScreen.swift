import SwiftUI

/// The roster of joined shares, and the home + switcher in one surface: the
/// wordmark, a "YOUR SHARES" section, cards (avatar · status · name), and a
/// floating add button. Tapping a card opens/resumes its browser (a warm session
/// shows a live marker); the trailing ⓘ opens the detail screen (info + Remove).
struct ShareListScreen: View {
    @Environment(ShareStore.self) private var store
    @Environment(ShareManager.self) private var manager
    @Environment(ShareIconStore.self) private var icons
    @Environment(BrowseRouter.self) private var router
    @State private var showingAdd = false

    var body: some View {
        ZStack(alignment: .bottomTrailing) {
            AccessColor.background.ignoresSafeArea()
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    logo
                    sectionHeader
                        .padding(.top, 40)
                    Group {
                        if store.shares.isEmpty {
                            empty
                        } else {
                            shareCards
                        }
                    }
                    .padding(.top, 12)
                }
                .padding(.horizontal, 24)
                .padding(.top, 24)
                .padding(.bottom, 96)
            }
            addButton
        }
        .toolbar(.hidden, for: .navigationBar)
        .sheet(isPresented: $showingAdd, onDismiss: consumePendingOpen) { AddShareScreen() }
        .task(id: store.shares.map(\.id)) {
            while !Task.isCancelled {
                await manager.status.refresh(store.shares.map(\.id), using: manager.sessions)
                try? await Task.sleep(for: .seconds(30))
            }
        }
    }

    private var logo: some View {
        Image("WispersAccessLogo")
            .renderingMode(.original)
            .resizable()
            .scaledToFit()
            .frame(height: 44)
            .frame(maxWidth: .infinity)
    }

    private var sectionHeader: some View {
        HStack {
            Text("YOUR SHARES")
                .font(.caption.weight(.medium)).tracking(1.5)
            Spacer()
            Text("\(store.shares.count)")
                .font(.caption.weight(.medium))
        }
        .foregroundStyle(AccessColor.onSurfaceVariant)
    }

    private var shareCards: some View {
        VStack(spacing: 12) {
            ForEach(store.shares) { share in
                shareRow(share)
            }
        }
    }

    /// One roster row: the card taps through to the browser; the trailing ⓘ taps
    /// through to the detail screen. Two side-by-side hit targets inside one card.
    private func shareRow(_ share: ShareMetadata) -> some View {
        HStack(spacing: 8) {
            NavigationLink(value: ShareRoute.browse(share.id)) {
                ShareCard(
                    share: share,
                    availability: manager.status.availability(for: share.id),
                    isLive: manager.browser.isWarm(share.id),
                    iconData: icons.iconData(for: share.id)
                )
            }
            .buttonStyle(.plain)

            NavigationLink(value: ShareRoute.detail(share.id)) {
                Image(systemName: "info.circle")
                    .font(.title3)
                    .foregroundStyle(AccessColor.onSurfaceVariant)
                    .frame(width: 44, height: 44)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Share details")
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 16)
        .background(AccessColor.surface, in: RoundedRectangle(cornerRadius: 16, style: .continuous))
    }

    private var empty: some View {
        Text("No shares yet. Tap + to add one.")
            .font(.subheadline)
            .foregroundStyle(AccessColor.onSurfaceVariant)
            .frame(maxWidth: .infinity)
            .padding(.top, 80)
    }

    private var addButton: some View {
        Button { showingAdd = true } label: {
            Image(systemName: "plus")
                .font(.title2.weight(.semibold))
                .foregroundStyle(AccessColor.primaryDark)
                .frame(width: 56, height: 56)
                .background(AccessColor.primary, in: Circle())
                .shadow(color: .black.opacity(0.15), radius: 5, y: 3)
        }
        .padding(24)
    }

    /// After the add sheet closes, open the freshly joined share if it asked to —
    /// deferred to here so we don't push onto the stack while the sheet is up.
    private func consumePendingOpen() {
        if let id = router.openAfterDismiss {
            router.openAfterDismiss = nil
            router.open(id)
        }
    }
}

private struct ShareCard: View {
    let share: ShareMetadata
    let availability: Availability
    let isLive: Bool
    let iconData: Data?

    var body: some View {
        HStack(spacing: 16) {
            ShareAvatar(nickname: name, iconPNG: iconData, size: 48)
                .overlay(alignment: .topTrailing) {
                    if isLive { liveBadge }
                }
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 8) {
                    StatusDot(availability: availability)
                    Text(statusLine)
                        .font(.caption2.weight(.medium)).tracking(1)
                        .foregroundStyle(AccessColor.onSurfaceVariant)
                }
                Text(name)
                    .font(.system(.title3, design: .serif).weight(.bold))
                    .foregroundStyle(AccessColor.onSurface)
                    .fixedSize(horizontal: false, vertical: true)
                    .multilineTextAlignment(.leading)
            }
            Spacer(minLength: 8)
        }
        .contentShape(Rectangle())
    }

    /// Subtle presence badge on the avatar: this share has a warm session open.
    private var liveBadge: some View {
        Circle()
            .fill(AccessColor.primaryDark)
            .frame(width: 13, height: 13)
            .overlay(Circle().stroke(AccessColor.surface, lineWidth: 2.5))
            .offset(x: 3, y: -3)
            .accessibilityLabel("Open")
    }

    private var name: String {
        share.nickname.isEmpty ? "Untitled share" : share.nickname
    }

    private var statusLine: String {
        let status: String
        switch availability {
        case .online: status = "ONLINE"
        case .offline: status = "OFFLINE"
        case .checking: status = "CHECKING…"
        }
        guard let last = share.lastConnectedAt, let ago = Self.shortAgo(last) else { return status }
        return "\(status) · LAST \(ago) AGO"
    }

    /// Compact "5W" / "3D" / "2H" / "10M" since `date`, or nil if just now.
    private static func shortAgo(_ date: Date) -> String? {
        let minutes = Int(max(0, Date().timeIntervalSince(date)) / 60)
        switch minutes {
        case ..<1: return nil
        case ..<60: return "\(minutes)M"
        case ..<(60 * 24): return "\(minutes / 60)H"
        case ..<(60 * 24 * 7): return "\(minutes / (60 * 24))D"
        default: return "\(minutes / (60 * 24 * 7))W"
        }
    }
}
