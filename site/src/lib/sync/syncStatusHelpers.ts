import type { IconDefinition } from "@fortawesome/free-solid-svg-icons";
import {
  faCloud,
  faSync,
  faExclamationCircle,
} from "@fortawesome/free-solid-svg-icons";
import type { SyncState, SyncStatus } from "./store.svelte";

/// Null means hide the button (sync off).
export function iconForState(
  state: SyncState | undefined,
): IconDefinition | null {
  switch (state) {
    case "starting":
    case "syncing":
      return faSync;
    case "online":
      return faCloud;
    case "error":
      return faExclamationCircle;
    default:
      return null;
  }
}

export function isVisible(state: SyncState | undefined): boolean {
  return !!state && state !== "disabled";
}

export function isSpinning(state: SyncState | undefined): boolean {
  return state === "starting" || state === "syncing";
}

export function tooltipFor(status: SyncStatus | undefined): string {
  switch (status?.state) {
    case "starting":
      return "Sync starting…";
    case "syncing":
      return status.completion != null
        ? `Syncing ${Math.floor(status.completion)}%`
        : "Syncing…";
    case "online":
      return status.deviceCount === 0
        ? "Sync on — no devices paired"
        : `Sync on — ${status.connectedCount}/${status.deviceCount} connected`;
    case "error":
      return status.lastError
        ? `Sync error: ${status.lastError}`
        : "Sync error";
    default:
      return "";
  }
}

/// One warning per non-zero count; an absent field means no issues. The shared
/// identity is not listed: it carries actions, see `sharesIdentity`.
export function storeIssueMessages(status: SyncStatus | undefined): string[] {
  const issues = status?.storeIssues;
  if (!issues) return [];
  const out: string[] = [];
  if (issues.newerVersion > 0 || issues.stalled)
    out.push(
      "Another device uses a newer version of the app. Update this device to see its latest changes.",
    );
  if (issues.unreadable > 0)
    out.push(
      "Some synced data could not be read. Changes from another device may be missing.",
    );
  if (issues.strangers > 0)
    out.push(
      "The sync folder contains conflict copies that are being ignored.",
    );
  return out;
}

export const SHARED_IDENTITY_MESSAGE =
  "This device shares its sync identity with another copy of the app, as happens after restoring a backup. One of the two needs a new identity and will then have to be paired again.";

export function sharesIdentity(status: SyncStatus | undefined): boolean {
  return status?.storeIssues?.sharedIdentity === true;
}
