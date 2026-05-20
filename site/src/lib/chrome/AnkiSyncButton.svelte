<script lang="ts">
    import Fa from 'svelte-fa';
    import {
        iconForState,
        isVisible,
        isSpinning,
        isClickDisabled,
        triggerSyncNow,
    } from './ankiSyncHelpers';
    import { ankiSyncStatus } from './ankiSyncStore.svelte';

    let status = $derived(ankiSyncStatus.current);
    let state = $derived(status?.state ?? 'unreachable');
    let icon = $derived(iconForState(state));
    let visible = $derived(isVisible(state));
    let spinning = $derived(isSpinning(state));
    let disabled = $derived(isClickDisabled(state));
    let tooltip = $derived(
        state === 'err' && status?.lastError ? status.lastError : '',
    );

    async function onClick() {
        try {
            await triggerSyncNow();
        } catch {
            // Backend pushes Err / Unreachable status through the watch
            // sender; UI updates via the anki_sync_status_changed event.
            // Nothing to do here.
        }
    }
</script>

{#if visible && icon}
    <button
        type="button"
        class="anki-sync-button"
        class:spin={spinning}
        {disabled}
        title={tooltip}
        onclick={onClick}
        data-testid="anki-sync-button"
    >
        <Fa {icon} />
        <span class="label">Sync Anki</span>
    </button>
{/if}

<style>
    .anki-sync-button {
        display: inline-flex;
        align-items: center;
        gap: 6px;
        background: none;
        border: none;
        padding: 6px 12px;
        cursor: pointer;
        color: var(--text-inverted, inherit);
        font-size: 0.9em;
    }
    .anki-sync-button:disabled {
        cursor: not-allowed;
        opacity: 0.7;
    }
    .anki-sync-button.spin :global(svg) {
        animation: anki-sync-spin 0.9s linear infinite;
    }
    @keyframes anki-sync-spin {
        from {
            transform: rotate(0deg);
        }
        to {
            transform: rotate(360deg);
        }
    }
    .label {
        font-size: inherit;
    }
</style>
