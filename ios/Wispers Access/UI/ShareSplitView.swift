import SwiftUI
import WispersAccessSdk

/// The iPad main window, after the desktop app: the shares in a sidebar, the
/// selected one's detail beside it, or the join form while there are none.
/// Apps open from the detail in windows of their own, which the OS switches
/// between. In a narrow window the split collapses to a stack, sidebar first.
struct ShareSplitView: View {
    @Environment(ShareManager.self) private var manager
    @Environment(BrowseRouter.self) private var router
    @State private var showingAdd = DemoMode.presentAddSheet
    @State private var compactColumn: NavigationSplitViewColumn = .sidebar

    var body: some View {
        NavigationSplitView(columnVisibility: .constant(.all), preferredCompactColumn: $compactColumn) {
            ShareSidebar(onSelect: select, onAdd: { showingAdd = true })
                .navigationSplitViewColumnWidth(300)
                .toolbar(removing: .sidebarToggle)
        } detail: {
            detail
        }
        .navigationSplitViewStyle(.balanced)
        .sheet(isPresented: $showingAdd) {
            AddShareScreen(onJoined: { select($0.id) })
        }
        .onChange(of: manager.shares.map(\.id), initial: true) { _, ids in
            keepSelection(within: ids)
        }
        .task(id: manager.shares.map(\.id)) {
            while !Task.isCancelled {
                await manager.status.refresh(
                    manager.shares, using: manager.client, activity: manager.activity)
                try? await Task.sleep(for: .seconds(30))
            }
        }
    }

    @ViewBuilder private var detail: some View {
        if let id = router.selection, manager.share(id) != nil {
            ShareDetailScreen(shareID: id)
                .id(id)
        } else if manager.shares.isEmpty {
            FirstRunPane(onJoined: { select($0.id) })
        }
    }

    /// Shows a share's detail — in a collapsed split that means pushing it.
    private func select(_ id: ShareId) {
        router.selection = id
        compactColumn = .detail
    }

    /// Like the desktop, something is always selected while there are shares:
    /// the first one when nothing is yet, or the selected one went away.
    private func keepSelection(within ids: [ShareId]) {
        if let selection = router.selection, !ids.contains(selection) {
            router.selection = nil
        }
        if router.selection == nil {
            router.selection = ids.first
        }
    }
}

/// The sidebar: wordmark, the shares one row each, and the add button pinned
/// to the bottom. A share's apps are in its detail, not here.
private struct ShareSidebar: View {
    let onSelect: (ShareId) -> Void
    let onAdd: () -> Void

    @Environment(ShareManager.self) private var manager
    @Environment(BrowseRouter.self) private var router

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Image("WispersAccessLogo")
                .renderingMode(.original)
                .resizable()
                .scaledToFit()
                .frame(width: 150)
                .padding(.horizontal, 10)
                .accessibilityLabel("Wispers Access")
            header
                .padding(.top, 28)
            ScrollView {
                VStack(spacing: 4) {
                    if manager.shares.isEmpty {
                        Text("Nothing yet. Shares you join appear here.")
                            .font(.subheadline)
                            .foregroundStyle(AccessColor.onSurfaceVariant)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(.horizontal, 10)
                    }
                    ForEach(manager.shares) { share in
                        Button { onSelect(share.id) } label: {
                            ShareRow(share: share, isSelected: share.id == router.selection)
                        }
                        .buttonStyle(.plain)
                    }
                }
                .padding(.top, 12)
            }
            addButton
                .padding(.top, 12)
        }
        .padding(.horizontal, 14)
        .padding(.top, 20)
        .padding(.bottom, 16)
        .background(AccessColor.sidebar)
        .toolbar(.hidden, for: .navigationBar)
    }

    private var header: some View {
        HStack {
            Text("SHARED WITH YOU")
            Spacer()
            if !manager.shares.isEmpty {
                Text("\(manager.shares.count)")
            }
        }
        .font(.caption.weight(.semibold)).tracking(1.5)
        .foregroundStyle(AccessColor.onSurfaceVariant)
        .padding(.horizontal, 10)
    }

    private var addButton: some View {
        Button(action: onAdd) {
            Label("Add a share", systemImage: "plus")
                .font(.body.weight(.semibold))
                .foregroundStyle(AccessColor.primaryDark)
                .frame(maxWidth: .infinity)
                .frame(height: 44)
                .background(AccessColor.primary, in: Capsule())
        }
        .buttonStyle(.plain)
        .keyboardShortcut("n")
    }
}

/// One share in the sidebar: status dot and name, then its app count, status
/// and last contact, with its apps' icons overlapping on the right. The
/// selected row is a card; an unreachable one is dimmed.
private struct ShareRow: View {
    let share: Share
    let isSelected: Bool

    @Environment(ShareManager.self) private var manager
    @Environment(ShareIconStore.self) private var icons

    var body: some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 8) {
                    StatusDot(availability: availability)
                    Text(share.name.isEmpty ? "Untitled share" : share.name)
                        .font(.system(size: 17, weight: .bold, design: .serif))
                        .foregroundStyle(AccessColor.onSurface)
                        .lineLimit(1)
                }
                Text(summary)
                    .font(.caption)
                    .foregroundStyle(AccessColor.onSurfaceVariant)
                    .lineLimit(1)
                    .padding(.leading, 16)
            }
            Spacer(minLength: 0)
            appIcons
        }
        .padding(12)
        .background {
            if isSelected {
                RoundedRectangle(cornerRadius: 12, style: .continuous)
                    .fill(AccessColor.surface)
                    .shadow(color: .black.opacity(0.06), radius: 1, y: 1)
            }
        }
        .opacity(isDimmed && !isSelected ? 0.6 : 1)
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(isSelected ? .isSelected : [])
    }

    /// Up to three of the share's app icons, each ringed in the row's
    /// background so the overlap reads as a stack.
    private var appIcons: some View {
        HStack(spacing: -6) {
            ForEach(share.apps.prefix(3), id: \.id) { app in
                ShareAvatar(
                    nickname: app.name,
                    iconPNG: icons.iconData(for: SharedAppId(shareID: share.id, appID: app.id)),
                    size: 20
                )
                .padding(2)
                .background(
                    isSelected ? AccessColor.surface : AccessColor.sidebar,
                    in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            }
        }
    }

    private var availability: Availability {
        share.state.availability ?? manager.status.availability(for: share.id)
    }

    private var isDimmed: Bool {
        availability == .offline || share.state != .live
    }

    /// "2 apps · online 5m ago".
    private var summary: String {
        let apps = share.apps.count == 1 ? "1 app" : "\(share.apps.count) apps"
        var line = "\(apps) · \(Self.describe(availability))"
        if share.state == .live, let last = manager.activity.lastConnected(share.id) {
            line += " \(Self.ago(last))"
        }
        return line
    }

    private static func describe(_ availability: Availability) -> String {
        switch availability {
        case .online: "online"
        case .offline: "offline"
        case .unknown: "unknown"
        case .checking: "checking…"
        case .removed, .revoked: "no longer available"
        }
    }

    /// "just now", "5m ago", "3h ago", "2d ago", "1w ago".
    private static func ago(_ date: Date) -> String {
        let minutes = Int(max(0, Date().timeIntervalSince(date)) / 60)
        switch minutes {
        case ..<1: return "just now"
        case ..<60: return "\(minutes)m ago"
        case ..<(60 * 24): return "\(minutes / 60)h ago"
        case ..<(60 * 24 * 7): return "\(minutes / (60 * 24))d ago"
        default: return "\(minutes / (60 * 24 * 7))w ago"
        }
    }
}

/// No shares yet: the join form inline, where the detail will be.
private struct FirstRunPane: View {
    let onJoined: (Share) -> Void
    @State private var phase: JoinPhase = .idle

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                Text("Join a share")
                    .font(.system(size: 32, weight: .bold, design: .serif))
                    .foregroundStyle(AccessColor.onSurface)
                Text("Paste the invitation code you were sent, or scan its QR code. The apps behind it show up on the left once you have joined.")
                    .font(.body)
                    .foregroundStyle(AccessColor.onSurfaceVariant)
                    .padding(.top, 12)
                    .padding(.bottom, 28)
                JoinShareForm(phase: $phase, onJoined: onJoined)
            }
            .frame(maxWidth: 400)
            .padding(.top, 120)
            .padding(.horizontal, 24)
            .frame(maxWidth: .infinity)
        }
        .background(AccessColor.background)
    }
}
