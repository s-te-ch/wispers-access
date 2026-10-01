// The native side's commands and the shapes they return.
// See src-tauri/src/shares.rs.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type ShareState = "live" | "removed" | "revoked";

export type AppKind = "web" | "jellyfin" | "immich";

export interface SharedApp {
  id: string;
  name: string;
  kind: AppKind;
  /** Where the browser finds the app, `<app>.<share>.wa.localhost:<port>`. */
  host: string;
}

export interface Share {
  id: string;
  name: string;
  /** The `<share>` in the apps' addresses. */
  label: string;
  transport: "iroh" | "wispers-connect" | "tailscale";
  apps: SharedApp[];
  state: ShareState;
  joinedAtMs: number;
  lastConnectedMs: number | null;
}

/** Share availability: reachable or not, or the host node turned us away for good. */
export type Availability = "online" | "offline" | "unknown" | "removed" | "revoked";

export interface ShareStatus {
  availability: Availability;
  lastConnectedMs: number | null;
}

/** Every joined share, from the store. Doesn't wait for the network. */
export const shares = () => invoke<Share[]>("shares");

/** Asks the host node for changes. This is also the reachability check behind
 * the status dot. */
export const checkShare = (shareId: string) => invoke<ShareStatus>("check_share", { shareId });

/** Opens an app in the system browser, pairing the browser on the way. */
export const openApp = (shareId: string, appId: string) =>
  invoke<void>("open_app", { shareId, appId });

/** The clipboard's text if it is an invite code. */
export const clipboardInvite = () => invoke<string | null>("clipboard_invite");

export const join = (inviteCode: string) => invoke<Share>("join", { inviteCode });

export const leave = (shareId: string) => invoke<void>("leave", { shareId });

/** Called whenever a stored share changed, so the list is worth reloading. */
export const onSharesChanged = (handler: () => void): Promise<UnlistenFn> =>
  listen("shares-changed", handler);
