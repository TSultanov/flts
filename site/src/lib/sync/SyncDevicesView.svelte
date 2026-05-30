<script lang="ts">
    import QRCode from "qrcode";
    import {
        syncStatus,
        syncSetEnabled,
        syncGetThisDevice,
        syncListDevices,
        syncAddDevice,
        syncRemoveDevice,
        canScan,
        scanDeviceId,
        type SyncStatus,
        type ThisDevice,
        type DeviceEntry,
    } from "./store.svelte";

    const scanAvailable = canScan();

    let status = $derived(syncStatus.current);
    let enabled = $derived(!!status && status.state !== "disabled");

    let thisDevice = $state<ThisDevice | null>(null);
    let qrDataUrl = $state("");
    let devices = $state<DeviceEntry[]>([]);
    let busy = $state(false);
    let error = $state("");

    let newId = $state("");
    let newName = $state("");

    function statusLabel(s: SyncStatus | undefined): string {
        switch (s?.state) {
            case "starting":
                return "Starting…";
            case "online":
                return s.deviceCount === 0
                    ? "Online — no devices paired yet"
                    : `Online — ${s.connectedCount}/${s.deviceCount} connected`;
            case "error":
                return "Error";
            default:
                return "Off";
        }
    }

    async function refresh() {
        try {
            thisDevice = await syncGetThisDevice();
            devices = await syncListDevices();
            qrDataUrl = thisDevice?.deviceId
                ? await QRCode.toDataURL(thisDevice.deviceId, { margin: 1, width: 220 })
                : "";
        } catch (e) {
            error = String(e);
        }
    }

    // Re-fetch identity + device list whenever sync state / device count moves
    // (the backend poller drives `sync_status_changed`). Memoized: no refetch
    // on unchanged polls.
    let refreshKey = $derived(`${status?.state}:${status?.deviceCount}`);
    $effect(() => {
        void refreshKey;
        refresh();
    });

    async function toggle() {
        busy = true;
        error = "";
        try {
            await syncSetEnabled(!enabled);
        } catch (e) {
            error = String(e);
        }
        busy = false;
    }

    async function add() {
        const id = newId.trim();
        if (!id) return;
        busy = true;
        error = "";
        try {
            await syncAddDevice(id, newName.trim() || "Device");
            newId = "";
            newName = "";
            await refresh();
        } catch (e) {
            error = String(e);
        }
        busy = false;
    }

    async function scanToAdd() {
        busy = true;
        error = "";
        try {
            const id = await scanDeviceId();
            if (id) newId = id;
        } catch (e) {
            error = String(e);
        }
        busy = false;
    }

    async function remove(id: string) {
        busy = true;
        error = "";
        try {
            await syncRemoveDevice(id);
            await refresh();
        } catch (e) {
            error = String(e);
        }
        busy = false;
    }

    async function copyId() {
        if (thisDevice?.deviceId) {
            await navigator.clipboard.writeText(thisDevice.deviceId);
        }
    }
</script>

<div class="sync">
    {#if !enabled}
        <p class="hint">
            Sync your library across your devices using the built-in engine — no
            separate app to install. If you previously synced FLTS with an
            external Syncthing, stop it first to avoid conflicts.
        </p>
        <button onclick={toggle} disabled={busy}>Enable sync</button>
    {:else}
        <div class="status-line">
            <span class="dot {status?.state}"></span>
            <span>{statusLabel(status)}</span>
            <button class="link" onclick={toggle} disabled={busy}>Disable</button>
        </div>

        {#if status?.state === "error" && status?.lastError}
            <p class="err">{status.lastError}</p>
        {/if}

        {#if thisDevice}
            <div class="this-device">
                {#if qrDataUrl}
                    <img src={qrDataUrl} alt="This device pairing QR code" />
                {/if}
                <div class="this-device-info">
                    <p class="label">This device</p>
                    <code class="id">{thisDevice.deviceId}</code>
                    <button onclick={copyId}>Copy ID</button>
                </div>
            </div>
        {/if}

        <p class="label">Pair a device</p>
        <p class="hint">
            Paste the other device's ID (shown in its sync settings or via its QR
            code), then add this device's ID there too.
        </p>
        <input placeholder="Device ID" bind:value={newId} />
        <input placeholder="Name (optional)" bind:value={newName} />
        <div class="add-actions">
            {#if scanAvailable}
                <button onclick={scanToAdd} disabled={busy}>Scan QR</button>
            {/if}
            <button onclick={add} disabled={busy || !newId.trim()}>Add device</button>
        </div>

        {#if devices.length > 0}
            <p class="label">Paired devices</p>
            <ul class="devices">
                {#each devices as d (d.deviceId)}
                    <li>
                        <span class="dot {d.connected ? 'online' : 'offline'}"></span>
                        <span class="name">{d.name || d.deviceId.slice(0, 7)}</span>
                        <button
                            class="link"
                            onclick={() => remove(d.deviceId)}
                            disabled={busy}>Remove</button
                        >
                    </li>
                {/each}
            </ul>
        {/if}

        {#if error}
            <p class="err">{error}</p>
        {/if}
    {/if}
</div>

<style>
    .sync {
        display: flex;
        flex-direction: column;
        gap: 8px;
    }

    .hint {
        margin: 0;
        font-size: 0.85em;
        opacity: 0.75;
    }

    .label {
        margin: 6px 0 0;
        font-weight: 600;
    }

    .status-line {
        display: flex;
        align-items: center;
        gap: 8px;
    }

    .dot {
        width: 9px;
        height: 9px;
        border-radius: 50%;
        background: #999;
        flex: none;
    }
    .dot.online {
        background: #3fb950;
    }
    .dot.starting {
        background: #d29922;
    }
    .dot.error,
    .dot.offline {
        background: #999;
    }

    .this-device {
        display: flex;
        gap: 12px;
        align-items: center;
    }
    .this-device img {
        width: 110px;
        height: 110px;
        border-radius: 6px;
        background: #fff;
    }
    .this-device-info {
        display: flex;
        flex-direction: column;
        gap: 6px;
        min-width: 0;
    }
    .id {
        font-size: 0.72em;
        word-break: break-all;
        opacity: 0.85;
    }

    .add-actions {
        display: flex;
        gap: 8px;
    }

    .devices {
        list-style: none;
        margin: 0;
        padding: 0;
        display: flex;
        flex-direction: column;
        gap: 4px;
    }
    .devices li {
        display: flex;
        align-items: center;
        gap: 8px;
    }
    .devices .name {
        flex: 1;
        min-width: 0;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    button.link {
        background: none;
        border: none;
        color: #4493f8;
        cursor: pointer;
        padding: 0;
        font: inherit;
    }
    button.link:disabled {
        opacity: 0.5;
        cursor: default;
    }

    .err {
        margin: 0;
        color: #f85149;
        font-size: 0.85em;
    }
</style>
