import type { IconDefinition } from "@fortawesome/free-solid-svg-icons";
import {
    faCloud,
    faSync,
    faExclamationCircle,
} from "@fortawesome/free-solid-svg-icons";
import type { SyncState, SyncStatus } from "./store.svelte";

/// Icon for a sync state; null = hide the button (sync off). Pure so it's
/// unit-testable without rendering.
export function iconForState(state: SyncState | undefined): IconDefinition | null {
    switch (state) {
        case "starting":
        case "syncing":
            return faSync;
        case "online":
            return faCloud;
        case "error":
            return faExclamationCircle;
        default:
            return null; // disabled / undefined
    }
}

export function isVisible(state: SyncState | undefined): boolean {
    return !!state && state !== "disabled";
}

export function isSpinning(state: SyncState | undefined): boolean {
    return state === "starting" || state === "syncing";
}

/// Short tooltip describing the current state.
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
            return status.lastError ? `Sync error: ${status.lastError}` : "Sync error";
        default:
            return "";
    }
}
