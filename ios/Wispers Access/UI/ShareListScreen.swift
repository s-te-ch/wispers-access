import SwiftUI
import WispersAccessSdk

/// The roster: the apps shared with you, grouped by the share they come from,
/// and the home + switcher in one surface. A share's header carries its name
/// and status and leads to its detail screen; each app underneath is a card
/// that opens (or resumes) its browser in one tap, a warm session showing a
/// live marker. A floating button adds a share.
struct ShareListScreen: View {
    @Environment(ShareManager.self) private var manager
    @Environment(ShareIconStore.self) private var icons
    @Environment(BrowseRouter.self) private var router
    @State private var showingAdd = DemoMode.presentAddSheet

    var body: some View {
        ZStack(alignment: .bottomTrailing) {
            AccessColor.background.ignoresSafeArea()
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    logo
                    sectionHeader
                        .padding(.top, 40)
                    Group {
                        if manager.shares.isEmpty {
                            empty
                        } else {
                            shareSections
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
        .task(id: manager.shares.map(\.id)) {
            while !Task.isCancelled {
                await manager.status.refresh(
                    manager.shares, using: manager.client, activity: manager.activity)
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
            Text("SHARED WITH YOU")
                .font(.caption.weight(.medium)).tracking(1.5)
            Spacer()
            Text("\(manager.shares.reduce(0) { $0 + $1.apps.count })")
                .font(.caption.weight(.medium))
        }
        .foregroundStyle(AccessColor.onSurfaceVariant)
    }

    private var shareSections: some View {
        VStack(spacing: 24) {
            ForEach(manager.shares) { share in
                shareSection(share)
            }
        }
    }

    /// One share: its header, then one card per app. A terminal share keeps
    /// its apps on screen, dimmed, and every tap on it leads to the detail
    /// screen, which explains and offers Remove.
    private func shareSection(_ share: Share) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            NavigationLink(value: ShareRoute.detail(share.id)) {
                ShareHeader(
                    share: share,
                    availability: share.state.availability
                        ?? manager.status.availability(for: share.id),
                    lastConnected: manager.activity.lastConnected(share.id)
                )
            }
            .buttonStyle(.plain)

            if share.apps.isEmpty {
                Text("No apps shared yet.")
                    .font(.subheadline)
                    .foregroundStyle(AccessColor.onSurfaceVariant)
                    .padding(.horizontal, 16)
                    .padding(.vertical, 12)
            }
            ForEach(share.apps, id: \.id) { app in
                let key = BrowseKey(shareID: share.id, appID: app.id)
                NavigationLink(
                    value: share.state == .live ? ShareRoute.browse(key) : ShareRoute.detail(share.id)
                ) {
                    AppCard(
                        app: app,
                        isLive: manager.browser.isWarm(key),
                        iconData: icons.iconData(for: key)
                    )
                    .opacity(share.state == .live ? 1 : 0.6)
                }
                .buttonStyle(.plain)
            }
        }
    }

    private var empty: some View {
        Text("No apps yet. Tap + to add a share.")
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
            if let share = manager.share(id) { router.open(share) }
        }
    }
}

/// A share's line above its apps: status dot and line, the name, and the ⓘ
/// that says the whole header leads to the detail screen.
private struct ShareHeader: View {
    let share: Share
    let availability: Availability
    let lastConnected: Date?

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 8) {
                    StatusDot(availability: availability)
                    Text(statusLine)
                        .font(.caption2.weight(.medium)).tracking(1)
                        .foregroundStyle(AccessColor.onSurfaceVariant)
                }
                Text(share.name.isEmpty ? "Untitled share" : share.name)
                    .font(.system(.title3, design: .serif).weight(.bold))
                    .foregroundStyle(AccessColor.onSurface)
                    .fixedSize(horizontal: false, vertical: true)
                    .multilineTextAlignment(.leading)
            }
            Spacer(minLength: 8)
            Image(systemName: "info.circle")
                .font(.title3)
                .foregroundStyle(AccessColor.onSurfaceVariant)
                .accessibilityLabel("Share details")
        }
        .padding(.horizontal, 8)
        .contentShape(Rectangle())
    }

    private var statusLine: String {
        let status: String
        switch availability {
        case .online: status = "ONLINE"
        case .offline: status = "OFFLINE"
        case .unknown: status = "UNKNOWN"
        case .checking: status = "CHECKING…"
        case .removed, .revoked: return "NO LONGER SHARED"
        }
        guard let lastConnected, let ago = Self.shortAgo(lastConnected) else { return status }
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

/// One app's card: its icon (harvested while browsing, else a letter tile)
/// and name. The thing the user taps.
private struct AppCard: View {
    let app: SharedApp
    let isLive: Bool
    let iconData: Data?

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
            Image(systemName: "chevron.right")
                .font(.footnote.weight(.semibold))
                .foregroundStyle(AccessColor.onSurfaceVariant)
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

    private var name: String {
        app.name.isEmpty ? app.id : app.name
    }
}
