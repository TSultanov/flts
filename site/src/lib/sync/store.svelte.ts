import { invoke } from "@tauri-apps/api/core";
import { platform } from "@tauri-apps/plugin-os";
import { Resource } from "../data/tauri.svelte";

export type SyncState = "disabled" | "starting" | "online" | "error";

export type SyncStatus = {
    state: SyncState;
    deviceId?: string;
    deviceCount: number;
    connectedCount: number;
    lastError?: string;
};

export type ThisDevice = { deviceId: string; name?: string };
export type DeviceEntry = { deviceId: string; name: string; connected: boolean };

/// Live sync status, refreshed whenever the backend emits `sync_status_changed`.
export const syncStatus = new Resource<SyncStatus>(
    "get_sync_status",
    {},
    [{ name: "sync_status_changed", filter: () => true }],
    { state: "disabled", deviceCount: 0, connectedCount: 0 },
);

export async function syncSetEnabled(enabled: boolean): Promise<void> {
    await invoke("sync_set_enabled", { enabled });
}

export async function syncGetThisDevice(): Promise<ThisDevice | null> {
    return await invoke<ThisDevice | null>("sync_get_this_device");
}

export async function syncListDevices(): Promise<DeviceEntry[]> {
    return await invoke<DeviceEntry[]>("sync_list_devices");
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

/// Opens the camera to scan a peer's pairing QR and returns its device ID, or
/// null if cancelled. Mobile only.
export async function scanDeviceId(): Promise<string | null> {
    const { scan, Format } = await import("@tauri-apps/plugin-barcode-scanner");
    // The native scanner renders the camera behind the webview; hide app chrome
    // while it's active.
    document.body.classList.add("barcode-scanning");
    try {
        const result = await scan({ windowed: true, formats: [Format.QRCode] });
        return extractDeviceId(result.content);
    } finally {
        document.body.classList.remove("barcode-scanning");
    }
}

/// The QR encodes the bare device ID (optionally a `{deviceId,name}` JSON blob);
/// accept either.
function extractDeviceId(content: string): string {
    const trimmed = content.trim();
    try {
        const obj = JSON.parse(trimmed);
        if (obj && typeof obj.deviceId === "string") return obj.deviceId;
    } catch {
        // not JSON — treat as a raw device ID
    }
    return trimmed;
}
