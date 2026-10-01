// Share management.

import * as api from "./api";
import type { Availability, Share } from "./api";

/** How often every share is checked while the window is visible. */
const CHECK_INTERVAL_MS = 30_000;

export class ShareManager {
  shares = $state<Share[]>([]);
  /** Absent = not checked yet. A terminal share's state wins over this. */
  statuses = $state<Record<string, Availability>>({});
  selectedId = $state<string | null>(null);
  loaded = $state(false);

  selected = $derived(this.shares.find((s) => s.id === this.selectedId) ?? null);

  /** Loads the list, follows changes, and checks the shares while the window is visible. */
  async start() {
    await api.onSharesChanged(() => this.reload());
    await this.reload();
    this.loaded = true;
    this.checkWhileVisible();
  }

  async reload() {
    this.shares = await api.shares();
    const ids = new Set(this.shares.map((s) => s.id));
    if (this.selectedId !== null && !ids.has(this.selectedId)) this.selectedId = null;
    if (this.selectedId === null && this.shares.length > 0) this.selectedId = this.shares[0].id;
  }

  select(id: string) {
    this.selectedId = id;
  }

  /** The dot and label for a share: terminal states are forever, the rest is the last check. */
  availability(share: Share): Availability | "checking" {
    if (share.state !== "live") return share.state;
    return this.statuses[share.id] ?? "checking";
  }

  async join(inviteCode: string): Promise<Share> {
    const share = await api.join(inviteCode);
    await this.reload();
    this.selectedId = share.id;
    void this.check(share.id);
    return share;
  }

  /** Drops the share from the list at once; the native side leaves it in the background. */
  async leave(id: string) {
    this.shares = this.shares.filter((s) => s.id !== id);
    if (this.selectedId === id) this.selectedId = this.shares[0]?.id ?? null;
    await api.leave(id);
    await this.reload();
  }

  openApp(shareId: string, appId: string) {
    return api.openApp(shareId, appId);
  }

  /** Checks every live share concurrently; one unreachable host node does not hold up the others. */
  async checkAll() {
    await Promise.all(this.shares.filter((s) => s.state === "live").map((s) => this.check(s.id)));
  }

  private async check(id: string) {
    try {
      const status = await api.checkShare(id);
      this.statuses[id] = status.availability;
      const share = this.shares.find((s) => s.id === id);
      if (share) share.lastConnectedMs = status.lastConnectedMs;
    } catch (e) {
      console.warn(`checking share ${id} failed`, e);
      this.statuses[id] = "unknown";
    }
  }

  private checkWhileVisible() {
    let timer: ReturnType<typeof setInterval> | null = null;
    const run = () => {
      if (document.visibilityState === "visible") {
        void this.checkAll();
        timer ??= setInterval(() => void this.checkAll(), CHECK_INTERVAL_MS);
      } else if (timer !== null) {
        clearInterval(timer);
        timer = null;
      }
    };
    document.addEventListener("visibilitychange", run);
    run();
  }
}
