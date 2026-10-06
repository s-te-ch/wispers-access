# Wispers Access desktop

The desktop client app has a UI for joining and managing shares, much like the
client apps for Android and iOS. The difference is that it opens the shared web
apps in the system browser (Chrome, Safari,...), not an embedded WebView. This
lets users treat web apps shared through Wispers Access almost exactly like
normal websites.

Implemented with Tauri 2, using the Wispers Access SDK (`src-tauri/`) for
proxying and peer-to-peer connectivity, and SvelteKit + Tailwind for the UI
(`src/`).

```sh
npm install
npm run tauri dev      # the window, rebuilt on every change
npm run check          # svelte-check
npm run tauri build    # the bundle under src-tauri/target/release/bundle
```

The native side keeps one SDK client and one loopback proxy on
`wa.localhost:11235` (any free port if that one is taken) for the app's lifetime,
which outlasts a closed window: the dock icon on macOS and the tray icon on
Windows bring it back. The app also launches at login in the background, by
default. The "Launch at Login" item in the application menu on macOS and in the
tray menu on Windows lets you turn that off. Secrets go to the platform
credential store, the macOS Keychain or the Windows Credential Manager
(`src-tauri/src/secrets.rs`); state under the app's data directory,
`~/Library/Application Support/dev.wispers.access.desktop` on macOS and
`%APPDATA%\dev.wispers.access.desktop` on Windows.

Release instructions are in the Wispers monorepo, at docs/access/releases.md.