import SwiftUI

/// The app's root: the share list (which pushes detail), with the full-screen
/// browser presented over it. The browser stays alive across dismissals — its
/// open sessions persist in `manager.browser`.
struct RootView: View {
    @Environment(ShareManager.self) private var manager

    var body: some View {
        NavigationStack {
            ShareListScreen()
        }
        .fullScreenCover(isPresented: presented) {
            BrowserView()
        }
    }

    private var presented: Binding<Bool> {
        Binding(
            get: { manager.browser.isPresented },
            set: { manager.browser.isPresented = $0 }
        )
    }
}
