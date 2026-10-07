// Release the desktop app for this platform, start to finish: build, on
// macOS notarize the DMG, then add this platform's entry to latest.json.
//
// Needs the updater's private key (TAURI_SIGNING_PRIVATE_KEY, defaulting to
// the file `tauri signer generate` wrote) and its password, plus the
// platform's code signing: the Developer ID certificate in the login keychain
// and APPLE_ID / APPLE_PASSWORD / APPLE_TEAM_ID for notarization on macOS,
// the Authenticode certificate that tauri.windows.conf.json names on Windows.
// Linux has no code signing, except for the Tauri updater's signing.
// Variables can come from the git-ignored .env.release; without the key's
// password there or in the environment, the build asks for it.

import { execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join } from "node:path";
import { platform } from "node:process";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));

const envFile = join(here, ".env.release");
if (existsSync(envFile)) {
  for (const line of readFileSync(envFile, "utf8").split(/\r?\n/)) {
    const match = line.match(/^\s*([A-Za-z_][A-Za-z0-9_]*)=(.*)$/);
    if (match) process.env[match[1]] = match[2].trim().replace(/^(["'])(.*)\1$/, "$2");
  }
}

// The variable takes a path or the key itself.
const key = (process.env.TAURI_SIGNING_PRIVATE_KEY ||= join(homedir(), ".tauri", "wispers-access-desktop.key"));
if (!existsSync(key) && !key.startsWith("untrusted")) {
  console.error(`no updater key at ${key} (TAURI_SIGNING_PRIVATE_KEY overrides)`);
  process.exit(1);
}

function run(command, args) {
  // npm is npm.cmd on Windows, which only a shell runs.
  execFileSync(command, args, { cwd: here, stdio: "inherit", shell: platform === "win32" });
}

run("npm", ["run", "tauri", "build"]);
if (platform === "darwin") run("./notarize-dmg.sh", []);
run("node", ["updater-manifest.mjs"]);
