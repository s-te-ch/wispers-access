// Adds the update bundle `tauri build` just made to latest.json, the manifest
// installed apps poll for updates, and says what to publish where.
//
// The URLs point at the GitHub release for the version, which must exist
// with the bundles attached before the manifest lands on main.

import { execFileSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { basename, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { arch, platform } from "node:process";

const here = dirname(fileURLToPath(import.meta.url));
const manifestPath = join(here, "latest.json");

const metadata = JSON.parse(
  execFileSync("cargo", ["metadata", "--format-version", "1", "--no-deps", "--manifest-path", join(here, "src-tauri/Cargo.toml")], { encoding: "utf8" }),
);
const version = metadata.packages.find((p) => p.name === "wispers-access-desktop").version;
const bundleDir = join(metadata.target_directory, "release", "bundle");

/** The update bundle and the platform key the updater looks it up under. */
function hostBundle() {
  switch (platform) {
    case "darwin": {
      const dir = join(bundleDir, "macos");
      const file = readdirSync(dir).find((f) => f.endsWith(".app.tar.gz"));
      return file && { key: `darwin-${arch === "arm64" ? "aarch64" : "x86_64"}`, bundle: join(dir, file) };
    }
    case "win32": {
      const dir = join(bundleDir, "nsis");
      const file = existsSync(dir) && readdirSync(dir).find((f) => f.endsWith("-setup.exe"));
      return file && { key: `windows-${arch === "arm64" ? "aarch64" : "x86_64"}`, bundle: join(dir, file) };
    }
    default:
      return undefined;
  }
}

const host = hostBundle();
if (!host || !existsSync(`${host.bundle}.sig`)) {
  console.error(`no signed update bundle under ${bundleDir}; build with the updater key set`);
  process.exit(1);
}

// The bundler only warns when the private key isn't the public key's half,
// and installed apps would reject the update.
const signature = readFileSync(`${host.bundle}.sig`, "utf8").trim();
const config = JSON.parse(readFileSync(join(here, "src-tauri", "tauri.conf.json"), "utf8"));
if (keyId(signature) !== keyId(config.plugins.updater.pubkey)) {
  console.error(`${host.bundle}.sig is not by the key in tauri.conf.json; build with the updater key`);
  process.exit(1);
}

/** The key ID in a base64'd minisign key or signature file. */
function keyId(file) {
  const line = Buffer.from(file, "base64").toString("utf8").split("\n")[1];
  return Buffer.from(line, "base64").subarray(2, 10).toString("hex");
}

const release = `desktop-v${version}`;
// GitHub turns the spaces in asset names into dots.
const asset = basename(host.bundle).replaceAll(" ", ".");

let manifest = existsSync(manifestPath) ? JSON.parse(readFileSync(manifestPath, "utf8")) : {};
if (manifest.version !== version) {
  // Each platform's machine adds its own entry, so the other's must already
  // be in this checkout when it was first.
  console.warn(`latest.json is for ${manifest.version}, starting it over for ${version}. If another platform has released ${version} already, pull main and run this again.`);
  manifest = { version, platforms: {} };
}
// The first platform's machine creates the release, the next ones add to it.
const releaseExists = Object.keys(manifest.platforms).some((key) => key !== host.key);
manifest.pub_date = new Date().toISOString().replace(/\.\d{3}Z$/, "Z");
manifest.platforms[host.key] = {
  url: `https://github.com/s-te-ch/wispers-access/releases/download/${release}/${asset}`,
  signature,
};
writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);

const extra = platform === "darwin" ? readdirSync(join(bundleDir, "dmg")).filter((f) => f.endsWith(".dmg")).map((f) => join(bundleDir, "dmg", f)) : [];
const files = [...extra, host.bundle, `${host.bundle}.sig`].map((f) => `"${f}"`).join(" ");
const publish = releaseExists
  ? `gh release upload ${release} ${files}`
  : `gh release create ${release} --title "Wispers Access desktop ${version}" --generate-notes ${files}`;
console.log(`
Wrote desktop/latest.json for ${version} (${Object.keys(manifest.platforms).join(", ")}). To publish:

  ${publish}

then commit desktop/latest.json to main.
`);
