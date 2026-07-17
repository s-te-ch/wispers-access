import SwiftUI
import WebKit

/// Full-screen browser over the open sessions. Every open share's web view is
/// mounted (only the active one visible), so switching is instant and none are
/// torn down. A bottom bar carries Done / reload / a tab switcher.
struct BrowserView: View {
    @Environment(ShareManager.self) private var manager
    @State private var showingTabs = false

    var body: some View {
        let browser = manager.browser
        ZStack {
            Color(.systemBackground).ignoresSafeArea()

            ForEach(browser.sessions) { session in
                SessionWebView(session: session)
                    .opacity(session.shareID == browser.activeShareID ? 1 : 0)
                    .allowsHitTesting(session.shareID == browser.activeShareID)
                    .ignoresSafeArea(.container, edges: .bottom)
            }

            if let active = browser.activeSession {
                if let error = active.startupError {
                    ProxyErrorView(message: error) { active.start() }
                } else if active.isLoading {
                    ConnectingOverlay()
                }
            }
        }
        .safeAreaInset(edge: .bottom) { toolbar(browser) }
        .sheet(isPresented: $showingTabs) { TabOverview() }
    }

    private func toolbar(_ browser: BrowseSessionStore) -> some View {
        HStack(spacing: 20) {
            Button("Done") { browser.isPresented = false }
                .foregroundStyle(AccessColor.primaryDark)

            Spacer()

            Text(browser.activeSession?.name ?? "")
                .font(.subheadline.weight(.medium))
                .foregroundStyle(AccessColor.onSurface)
                .lineLimit(1)

            Spacer()

            Button { browser.activeSession?.reload() } label: {
                Image(systemName: "arrow.clockwise")
            }
            .foregroundStyle(AccessColor.primaryDark)
            .disabled(browser.activeSession == nil)

            Button { showingTabs = true } label: {
                HStack(spacing: 4) {
                    Image(systemName: "square.on.square")
                    Text("\(browser.sessions.count)")
                }
            }
            .foregroundStyle(AccessColor.primaryDark)
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 12)
        .background(.regularMaterial)
    }
}

/// Displays a session's retained `WKWebView` (never recreated, so page state
/// survives switching).
private struct SessionWebView: UIViewRepresentable {
    let session: BrowseSession

    func makeUIView(context: Context) -> WKWebView { session.webView }
    func updateUIView(_ uiView: WKWebView, context: Context) {}
}

/// The open-shares switcher: tap to switch, ✕ to close.
private struct TabOverview: View {
    @Environment(ShareManager.self) private var manager
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            List {
                ForEach(manager.browser.sessions) { session in
                    Button {
                        manager.browser.switchTo(session.shareID)
                        dismiss()
                    } label: {
                        HStack(spacing: 12) {
                            ShareAvatar(nickname: session.name, size: 36)
                            Text(session.name)
                                .foregroundStyle(AccessColor.onSurface)
                            Spacer()
                            if session.shareID == manager.browser.activeShareID {
                                Image(systemName: "checkmark")
                                    .foregroundStyle(AccessColor.primaryDark)
                            }
                            Button {
                                manager.browser.close(session.shareID)
                            } label: {
                                Image(systemName: "xmark.circle.fill")
                                    .foregroundStyle(AccessColor.onSurfaceVariant)
                            }
                            .buttonStyle(.plain)
                        }
                    }
                    .buttonStyle(.plain)
                }
            }
            .navigationTitle("Open shares")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
    }
}

private struct ConnectingOverlay: View {
    var body: some View {
        ZStack {
            Color(.systemBackground)
            VStack(spacing: 16) {
                ProgressView()
                Text("Connecting…").foregroundStyle(.secondary)
            }
        }
        .ignoresSafeArea()
    }
}

private struct ProxyErrorView: View {
    let message: String
    let onRetry: () -> Void

    var body: some View {
        ContentUnavailableView {
            Label("Connection problem", systemImage: "wifi.exclamationmark")
        } description: {
            Text(message)
        } actions: {
            Button("Retry", action: onRetry)
                .buttonStyle(.borderedProminent)
        }
    }
}
