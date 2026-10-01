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

const release = `desktop-v${version}`;
// GitHub turns the spaces in asset names into dots.
const asset = basename(host.bundle).replaceAll(" ", ".");

let manifest = existsSync(manifestPath) ? JSON.parse(readFileSync(manifestPath, "utf8")) : {};
if (manifest.version !== version) manifest = { version, platforms: {} };
manifest.pub_date = new Date().toISOString().replace(/\.\d{3}Z$/, "Z");
manifest.platforms[host.key] = {
  url: `https://github.com/s-te-ch/wispers-access/releases/download/${release}/${asset}`,
  signature: readFileSync(`${host.bundle}.sig`, "utf8").trim(),
};
writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);

const extra = platform === "darwin" ? readdirSync(join(bundleDir, "dmg")).filter((f) => f.endsWith(".dmg")).map((f) => join(bundleDir, "dmg", f)) : [];
const files = [...extra, host.bundle, `${host.bundle}.sig`].map((f) => `"${f}"`).join(" ");
console.log(`
Wrote desktop/latest.json for ${version} (${Object.keys(manifest.platforms).join(", ")}). To publish:

  gh release create ${release} --title "Wispers Access desktop ${version}" --generate-notes ${files}

(or \`gh release upload ${release} …\` if the release exists), then commit desktop/latest.json to main.
`);
