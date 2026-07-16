import SwiftUI

/// The app's root: the roster of joined shares, or an empty state prompting the
/// first join. The toolbar and empty state both open the add-share sheet.
struct RootView: View {
    @Environment(ShareStore.self) private var store
    @State private var showingAdd = false

    var body: some View {
        NavigationStack {
            Group {
                if store.shares.isEmpty {
                    ContentUnavailableView {
                        Label("No shares yet", systemImage: "square.grid.2x2")
                    } description: {
                        Text("Join a share with an invite code from whoever is hosting it.")
                    } actions: {
                        Button("Add share") { showingAdd = true }
                            .buttonStyle(.borderedProminent)
                    }
                } else {
                    ShareListScreen()
                }
            }
            .navigationTitle("Wispers Access")
            .toolbar {
                ToolbarItem(placement: .primaryAction) {
                    Button { showingAdd = true } label: {
                        Label("Add share", systemImage: "plus")
                    }
                }
            }
            .sheet(isPresented: $showingAdd) {
                AddShareScreen()
            }
        }
    }
}
