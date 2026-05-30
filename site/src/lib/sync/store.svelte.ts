import { invoke } from "@tauri-apps/api/core";
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
