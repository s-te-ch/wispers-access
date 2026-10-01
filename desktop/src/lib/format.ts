// Text for the UI's dates and states.

import type { Availability, Share } from "./api";

/** "2m ago", "3h ago", "1d ago", "2w ago"; "just now" under a minute. */
export function ago(ms: number, now = Date.now()): string {
  const minutes = Math.max(0, Math.floor((now - ms) / 60_000));
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  if (days < 7) return `${days}d ago`;
  return `${Math.floor(days / 7)}w ago`;
}

/** The ISO date, `2026-09-28`, as the mobile apps show it. */
export function isoDate(ms: number): string {
  return new Date(ms).toISOString().slice(0, 10);
}

export function describeAvailability(availability: Availability | "checking"): string {
  switch (availability) {
    case "online":
      return "online";
    case "offline":
      return "offline";
    case "unknown":
      return "unknown";
    case "checking":
      return "checking…";
    case "removed":
    case "revoked":
      return "no longer available";
  }
}

export function describeTransport(transport: Share["transport"]): string {
  switch (transport) {
    case "iroh":
      return "Peer to peer (iroh)";
    case "wispers-connect":
      return "Wispers Connect";
    case "tailscale":
      return "Tailscale";
  }
}

/** Why a terminal share is terminal, and what remains to do with it. */
export function explainTerminalState(state: Share["state"]): string {
  const what =
    state === "removed"
      ? "The share was removed by its host and can't be reached anymore."
      : "This device's access was revoked by the share's host.";
  return `${what} You can remove it from this device; joining again needs a new invitation code.`;
}

export const count = (n: number, noun: string) => `${n} ${noun}${n === 1 ? "" : "s"}`;
