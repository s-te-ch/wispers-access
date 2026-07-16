//
//  Wispers_AccessApp.swift
//  Wispers Access
//
//  Created by Matthias Scheidegger on 01.07.2026.
//

import SwiftUI

@main
struct Wispers_AccessApp: App {
    @State private var manager = ShareManager()

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(manager)
                .environment(manager.store)
        }
    }
}
