import SwiftUI

/// The list of joined shares. Each row shows the share's name and which backend
/// it lives on (managed vs. a named self-hosted hub). Swipe to remove.
struct ShareListScreen: View {
    @Environment(ShareStore.self) private var store
    @Environment(ShareManager.self) private var manager

    var body: some View {
        List {
            ForEach(store.shares) { share in
                NavigationLink {
                    BrowseScreen(share: share)
                } label: {
                    ShareRow(share: share)
                }
            }
            .onDelete(perform: delete)
        }
    }

    private func delete(_ offsets: IndexSet) {
        for index in offsets {
            manager.delete(store.shares[index].id)
        }
    }
}

private struct ShareRow: View {
    let share: ShareMetadata

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title)
                .font(.headline)
            backendLabel
                .font(.caption)
                .foregroundStyle(.secondary)
        }
        .padding(.vertical, 4)
    }

    @ViewBuilder
    private var backendLabel: some View {
        if let backend = share.backend {
            Label(host(of: backend), systemImage: "server.rack")
        } else {
            Label("Managed backend", systemImage: "cloud")
        }
    }

    private var title: String {
        share.nickname.isEmpty ? "Share \(share.id.value.prefix(8))" : share.nickname
    }

    private func host(of urlString: String) -> String {
        URL(string: urlString)?.host() ?? urlString
    }
}
