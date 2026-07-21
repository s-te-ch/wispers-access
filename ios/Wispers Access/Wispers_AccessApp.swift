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
    @State private var manager = DemoMode.active ? DemoMode.makeManager() : ShareManager()
    @State private var router = {
        let router = BrowseRouter()
        if let route = DemoMode.initialRoute { router.path = [route] }
        return router
    }()

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(manager)
                .environment(manager.store)
                .environment(manager.icons)
                .environment(router)
                .environment(QuickActionInbox.shared)
        }
    }
}
