// The native side's commands and the shapes they return.
// See src-tauri/src/shares.rs.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type ShareState = "live" | "outdated" | "removed" | "revoked";

export type AppKind = "web" | "jellyfin" | "immich";

export interface SharedApp {
  id: string;
  name: string;
  kind: AppKind;
  /** Where the browser finds the app: `http://<app>.<share>.wa.localhost:<port>/`
   * for a reverse-proxy app, the start URL for a forward-proxy app. */
  url: string;
  /** The problem description if this device can't open the app. */
  unavailable: string | null;
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
  /** Whether this app, as it started, had to quit and restart the browser
   * running the share's profile, because the share's port changed. Cleared
   * once one of the share's apps is opened. */
  profileRestarted: boolean;
  /** Whether leaving the share takes out a certificate this app installed
   * for it, which has the system ask the user. */
  removesCertificates: boolean;
}

/** Share availability: reachable or not, or the host node turned us away for good. */
export type Availability = "online" | "offline" | "unknown" | "outdated" | "removed" | "revoked";

export interface ShareStatus {
  availability: Availability;
  lastConnectedMs: number | null;
}

/** Every joined share, from the store. Doesn't wait for the network. */
export const shares = () => invoke<Share[]>("shares");

/** Asks the host node for changes. This is also the reachability check behind
 * the status dot. */
export const checkShare = (shareId: string) => invoke<ShareStatus>("check_share", { shareId });

/** Opens an app: a reverse-proxy app in the system browser, pairing the
 * browser on the way, a forward-proxy app in the share's browser profile. */
export const openApp = (shareId: string, appId: string) =>
  invoke<void>("open_app", { shareId, appId });

/** The clipboard's text if it is an invite code. */
export const clipboardInvite = () => invoke<string | null>("clipboard_invite");

export const join = (inviteCode: string) => invoke<Share>("join", { inviteCode });

export const leave = (shareId: string) => invoke<void>("leave", { shareId });

export interface Certificate {
  subject: string;
  kind: "ca" | "selfSignedServer" | "caSignedServer";
  /** A server certificate's names. */
  names: string[];
  /** Whether installing it can help: a CA's, or a self-signed server's. */
  installable: boolean;
  /** Whether this app installed it. */
  installed: boolean;
}

export interface CertificateStatus {
  /** The share's `https` origins whose certificate this device doesn't trust. */
  untrustedOrigins: string[];
  /**
   * Whether the check couldn't tell for any origin whether this device trusts it, as the
   * share's host node or every origin was out of reach.
   */
  unchecked: boolean;
  certificates: Certificate[];
  /** Whether this app can install certificates here, or only save them. */
  canInstall: boolean;
  /**
   * Whether the user saved the share's certificates. Whether they imported them in a browser
   * afterwards is out of the app's sight.
   */
  saved: boolean;
}

/** Checks the share's `https` origins by a TLS handshake through the share. */
export const checkCertificates = (shareId: string) =>
  invoke<CertificateStatus>("check_certificates", { shareId });

/** Installs the share's installable certificates; the system asks the user. */
export const installCertificates = (shareId: string) =>
  invoke<void>("install_certificates", { shareId });

/** Saves them into a folder the user picks: the folder, or null if cancelled. */
export const saveCertificates = (shareId: string) =>
  invoke<string | null>("save_certificates", { shareId });

/** What closing the window does: hide it, with the app running on in the
 * menu bar or tray, or quit, where nothing else holds the app. Then the window
 * carries the settings a menu would. */
export type WindowPolicy = "hideOnClose" | "quitOnClose";

export const windowPolicy = () => invoke<WindowPolicy>("window_policy");

/** Whether the app launches at login. */
export const autostartEnabled = () => invoke<boolean>("autostart_enabled");

export const setAutostartEnabled = (enabled: boolean) =>
  invoke<void>("set_autostart_enabled", { enabled });

/** Restarts the app, window showing. Used after a software update. */
export const restart = () => invoke<void>("restart");

/** Called whenever a stored share changed, so the list is worth reloading. */
export const onSharesChanged = (handler: () => void): Promise<UnlistenFn> =>
  listen("shares-changed", handler);
