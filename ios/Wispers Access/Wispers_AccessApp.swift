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
    @State private var manager = ShareManager()
    @State private var router = BrowseRouter()

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(manager)
                .environment(manager.store)
                .environment(router)
                .environment(QuickActionInbox.shared)
        }
    }
}
