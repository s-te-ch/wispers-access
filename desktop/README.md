# Wispers Access desktop

The desktop client app has a UI for joining and managing shares, much like the
client apps for Android and iOS. The difference is that it opens the shared web
apps in the system browser (Chrome, Safari,...), not an embedded WebView. This
lets users treat web apps shared through Wispers Access almost exactly like
normal websites.

The app is implemented with Tauri 2, using the Wispers Access SDK (`src-tauri/`)
for proxying and peer-to-peer connectivity, and SvelteKit + Tailwind for the UI
(`src/`).

```sh
npm install
npm run tauri dev      # the window, rebuilt on every change
npm run check          # svelte-check
npm run tauri build    # the bundle under src-tauri/target/release/bundle
```

The native side of the app keeps one SDK client and one loopback proxy on
`wa.localhost:11235` (or any free port if that one is taken) during the app's
lifetime. Closing the window does not stop the app, it keeps running the proxy
in the background. The dock icon (macOS) or the tray icon (Windows and Linux)
bring the window back (Exception: Linux desktops without a tray icon. There,
closing the window stop the app). The app also launches at login, by
default. The "Launch at Login" item (in the app or tray menus) lets you turn
this off. Secrets go to the platform credential store, or to files as a
fallback.

Linux builds need WebKitGTK 4.1 and libayatana-appindicator (on Debian and
Ubuntu, `libwebkit2gtk-4.1-dev` and `libayatana-appindicator3-dev`).

Release instructions are in the Wispers monorepo, at docs/access/releases.md.