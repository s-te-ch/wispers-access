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
`wa.localhost:4242` (any free port if that one is taken) for the window's
lifetime. Secrets go to the platform credential store, the macOS Keychain
today (`src-tauri/src/secrets.rs`); state under the app's data directory,
`~/Library/Application Support/dev.wispers.access.desktop` on macOS.

On macOS, `tauri dev` builds through `src-tauri/cargo-with-codesign.sh`, which
signs the binary with your Apple Development certificate before running it. The
Keychain recognises the app by its signature, so without this every rebuild
would prompt for every secret. Set `WISPERS_ACCESS_DEV_SIGNING_IDENTITY` to pick
another identity.
