// Auto-update handler for the Wispers Access desktop app itself.

import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

/** The app is long-lived (it hides rather than quits), so it looks again now and then. */
const CHECK_INTERVAL_MS = 6 * 60 * 60 * 1000;

export class UpdateChecker {
  /** The newer version, once found. */
  available = $state<Update | null>(null);
  installing = $state(false);
  /** Download progress, 0 to 1, while installing. */
  progress = $state(0);
  error = $state<string | null>(null);

  /** Checks now and periodically. A dev build has no published counterpart, so it skips. */
  start() {
    if (import.meta.env.DEV) return;
    void this.check();
    setInterval(() => void this.check(), CHECK_INTERVAL_MS);
  }

  async check() {
    try {
      const update = await check();
      if (update) this.available = update;
    } catch (e) {
      // Offline, or the manifest is unreachable: nothing the user can act on.
      console.warn("update check failed", e);
    }
  }

  /** Downloads, installs and relaunches. The app comes back as the new version. */
  async install() {
    if (!this.available || this.installing) return;
    this.installing = true;
    this.error = null;
    this.progress = 0;
    let total = 0;
    let received = 0;
    try {
      await this.available.downloadAndInstall((event) => {
        switch (event.event) {
          case "Started":
            total = event.data.contentLength ?? 0;
            break;
          case "Progress":
            received += event.data.chunkLength;
            if (total > 0) this.progress = Math.min(1, received / total);
            break;
          case "Finished":
            this.progress = 1;
            break;
        }
      });
      await relaunch();
    } catch (e) {
      this.error = String(e);
      this.installing = false;
    }
  }

  dismiss() {
    this.available = null;
  }
}
