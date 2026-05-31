<script lang="ts">
    import Fa from "svelte-fa";
    import { navigate } from "../../router";
    import { syncStatus } from "./store.svelte";
    import {
        iconForState,
        isVisible,
        isSpinning,
        tooltipFor,
    } from "./syncStatusHelpers";

    let status = $derived(syncStatus.current);
    let state = $derived(status?.state);
    let icon = $derived(iconForState(state));
    let visible = $derived(isVisible(state));
    let spinning = $derived(isSpinning(state));
    let tooltip = $derived(tooltipFor(status));
    // Show the percentage while syncing.
    let label = $derived(
        state === "syncing" && status?.completion != null
            ? `${Math.floor(status.completion)}%`
            : "",
    );

    function onClick() {
        navigate("/config");
    }
</script>

{#if visible && icon}
    <button
        type="button"
        class="sync-status-button"
        class:spin={spinning}
        class:error={state === "error"}
        title={tooltip}
        onclick={onClick}
        data-testid="sync-status-button"
    >
        <Fa {icon} />
        {#if label}<span class="pct">{label}</span>{/if}
    </button>
{/if}

<style>
    .sync-status-button {
        display: inline-flex;
        align-items: center;
        gap: 5px;
        background: none;
        border: none;
        padding: 6px 10px;
        cursor: pointer;
        color: var(--text-inverted, inherit);
        font-size: 0.9em;
    }
    .sync-status-button.error {
        color: #f85149;
    }
    .sync-status-button.spin :global(svg) {
        animation: sync-status-spin 0.9s linear infinite;
    }
    @keyframes sync-status-spin {
        from {
            transform: rotate(0deg);
        }
        to {
            transform: rotate(360deg);
        }
    }
    .pct {
        font-size: 0.85em;
        font-variant-numeric: tabular-nums;
    }
</style>
