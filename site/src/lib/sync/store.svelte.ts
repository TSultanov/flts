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

export const syncStatus = new Resource<SyncStatus>(
    "get_sync_status",
    {},
    [{ name: "sync_status_changed", filter: () => true }],
    { state: "disabled", deviceCount: 0, connectedCount: 0 },
);

// One-shot request to expand the Sync section: set by the nav status button,
// consumed by ConfigView. Reactive so it works on an already-open page too.
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

/// Non-null only in debug builds with the engine running — release ships
/// without Syncthing's web UI.
export async function syncWebUiUrl(): Promise<string | null> {
    return await invoke<string | null>("sync_web_ui_url");
}

/// On iOS the dashboard URL is shown for manual entry, never opened: Safari
/// would background the FLTS process that serves it.
export function isIos(): boolean {
    try {
        return platform() === "ios";
    } catch {
        return false;
    }
}

/// URL is validated backend-side.
export async function openExternalUrl(url: string): Promise<void> {
    await invoke("open_external_url", { url });
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

/// Mobile only; desktop pairs by paste.
export function canScan(): boolean {
    try {
        const p = platform();
        return p === "ios" || p === "android";
    } catch {
        return false;
    }
}

/// Must run before `scan()`: it does not request the camera permission
/// itself and fails outright on a fresh install.
export async function ensureCameraPermission(): Promise<boolean> {
    const { checkPermissions, requestPermissions } = await import(
        "@tauri-apps/plugin-barcode-scanner"
    );
    // The plugin's declared return type resolves to the DOM-global
    // PermissionState, which lacks 'prompt-with-rationale' — a value that
    // does occur at runtime on Android.
    type TauriPermissionState = import("@tauri-apps/api/core").PermissionState;
    let state = (await checkPermissions()) as TauriPermissionState;
    if (state === "prompt" || state === "prompt-with-rationale") {
        state = (await requestPermissions()) as TauriPermissionState;
    }
    return state === "granted";
}

/// Mobile only. The native scanner renders the camera *behind* the webview,
/// so the caller must make the page transparent while this runs (see
/// `barcode-scanning` in SyncDevicesView).
export async function scanDeviceId(): Promise<{ deviceId: string; name?: string } | null> {
    const { scan, Format } = await import("@tauri-apps/plugin-barcode-scanner");
    const result = await scan({ windowed: true, formats: [Format.QRCode] });
    return parsePairingPayload(result.content);
}

export async function cancelScan(): Promise<void> {
    const { cancel } = await import("@tauri-apps/plugin-barcode-scanner");
    await cancel();
}

/// Accepts either a `{deviceId,name}` JSON blob or a bare device ID.
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

export function pairingPayload(deviceId: string, name?: string): string {
    return JSON.stringify({ deviceId, name: name ?? "" });
}
