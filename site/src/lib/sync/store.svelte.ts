import { invoke } from "@tauri-apps/api/core";
import { platform } from "@tauri-apps/plugin-os";
import { Resource } from "../data/tauri.svelte";

export type SyncState = "disabled" | "starting" | "online" | "syncing" | "error";

export type SyncStatus = {
    state: SyncState;
    deviceId?: string;
    deviceCount: number;
    connectedCount: number;
    completion?: number;
    lastError?: string;
};

export type ThisDevice = { deviceId: string; name?: string };
export type DeviceEntry = { deviceId: string; name: string; connected: boolean };
export type PendingEntry = { deviceId: string; name: string };

/// Live sync status, refreshed whenever the backend emits `sync_status_changed`.
export const syncStatus = new Resource<SyncStatus>(
    "get_sync_status",
    {},
    [{ name: "sync_status_changed", filter: () => true }],
    { state: "disabled", deviceCount: 0, connectedCount: 0 },
);

// One-shot request to expand the Sync section in settings (set by the nav
// status button, consumed by ConfigView). Reactive so it works whether the
// config page is already open or navigated to.
let openSyncRequested = $state(false);

export function requestOpenSyncSection(): void {
    openSyncRequested = true;
}

/// Returns true once per request, then resets.
export function takeOpenSyncRequest(): boolean {
    if (openSyncRequested) {
        openSyncRequested = false;
        return true;
    }
    return false;
}

export async function syncSetEnabled(enabled: boolean): Promise<void> {
    await invoke("sync_set_enabled", { enabled });
}

export async function syncSetDeviceName(name: string): Promise<void> {
    await invoke("sync_set_device_name", { name });
}

export async function syncGetThisDevice(): Promise<ThisDevice | null> {
    return await invoke<ThisDevice | null>("sync_get_this_device");
}

export async function syncListDevices(): Promise<DeviceEntry[]> {
    return await invoke<DeviceEntry[]>("sync_list_devices");
}

export async function syncListPending(): Promise<PendingEntry[]> {
    return await invoke<PendingEntry[]>("sync_list_pending");
}

export async function syncAddDevice(deviceId: string, name: string): Promise<void> {
    await invoke("sync_add_device", { deviceId, name });
}

export async function syncRemoveDevice(deviceId: string): Promise<void> {
    await invoke("sync_remove_device", { deviceId });
}

/// Whether native camera QR scanning is available (mobile only; desktop pairs
/// by paste).
export function canScan(): boolean {
    try {
        const p = platform();
        return p === "ios" || p === "android";
    } catch {
        return false;
    }
}

/// Ensures the camera permission the QR scanner needs is granted, prompting the
/// user once if it hasn't been decided yet. Returns whether scanning may
/// proceed. `scan()` does NOT request the permission itself, so on a fresh
/// install it fails outright unless this runs first.
export async function ensureCameraPermission(): Promise<boolean> {
    const { checkPermissions, requestPermissions } = await import(
        "@tauri-apps/plugin-barcode-scanner"
    );
    let state = await checkPermissions();
    if (state === "prompt" || state === "prompt-with-rationale") {
        state = await requestPermissions();
    }
    return state === "granted";
}

/// Opens the camera to scan a peer's pairing QR and returns its device ID +
/// name, or null if cancelled. Mobile only.
///
/// The native scanner renders the camera *behind* the webview, so the caller is
/// responsible for making the page transparent while this runs (see the
/// `barcode-scanning` handling in SyncDevicesView).
export async function scanDeviceId(): Promise<{ deviceId: string; name?: string } | null> {
    const { scan, Format } = await import("@tauri-apps/plugin-barcode-scanner");
    const result = await scan({ windowed: true, formats: [Format.QRCode] });
    return parsePairingPayload(result.content);
}

/// Cancels an in-progress scan (the overlay's Cancel button).
export async function cancelScan(): Promise<void> {
    const { cancel } = await import("@tauri-apps/plugin-barcode-scanner");
    await cancel();
}

/// The QR encodes a `{deviceId,name}` JSON blob (or, for older codes, a bare
/// device ID); accept either.
export function parsePairingPayload(
    content: string,
): { deviceId: string; name?: string } | null {
    const trimmed = content.trim();
    if (!trimmed) return null;
    try {
        const obj = JSON.parse(trimmed);
        if (obj && typeof obj.deviceId === "string") {
            return { deviceId: obj.deviceId, name: typeof obj.name === "string" ? obj.name : undefined };
        }
    } catch {
        // not JSON — treat as a raw device ID
    }
    return { deviceId: trimmed };
}

/// The pairing QR payload for this device (id + name).
export function pairingPayload(deviceId: string, name?: string): string {
    return JSON.stringify({ deviceId, name: name ?? "" });
}
