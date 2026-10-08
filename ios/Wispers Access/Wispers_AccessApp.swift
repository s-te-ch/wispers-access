//
//  Wispers_AccessApp.swift
//  Wispers Access
//
//  Created by Matthias Scheidegger on 01.07.2026.
//

import SwiftUI

@main
struct Wispers_AccessApp: App {
    @UIApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate
    @Environment(\.scenePhase) private var scenePhase
    @State private var manager = DemoMode.active ? DemoMode.makeManager() : ShareManager.live()

    var body: some Scene {
        WindowGroup(id: RosterWindow.id) {
            RootView()
                .environment(manager)
                .environment(manager.icons)
                .environment(QuickActionInbox.shared)
        }
        // iPadOS's windowed mode opens a new window at a size of its choosing,
        // which can be narrow enough to collapse the sidebar layout into a
        // phone-like stack. Ask for room for the sidebar and the detail.
        .defaultSize(width: 1000, height: 760)
        // The app as a whole, not one window: keep the app-icon shortcuts
        // current — Apple's cue to refresh them — and let the SDK check its
        // connections after a longer stint away.
        .onChange(of: scenePhase) { _, phase in
            switch phase {
            case .background:
                UIApplication.shared.shortcutItems = QuickAction.shortcutItems(
                    for: manager.shares, activity: manager.activity)
                manager.wentToBackground()
            case .active:
                manager.cameToForeground()
            default:
                break
            }
        }

        // iPad only: each open app in a window of its own, opened from the
        // roster, so the OS switches between them like between any apps.
        WindowGroup(for: SharedAppId.self) { $key in
            AppWindow(key: key)
                .environment(manager)
                .environment(manager.icons)
        }
        // A web app wants a desktop-sized window, not a phone-sized one.
        .defaultSize(width: 1100, height: 800)
    }
}
