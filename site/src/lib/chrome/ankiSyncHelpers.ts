import { invoke } from '@tauri-apps/api/core';
import type { IconDefinition } from '@fortawesome/free-solid-svg-icons';
import {
    faSync,
    faCheck,
    faExclamationCircle,
} from '@fortawesome/free-solid-svg-icons';

export type AnkiSyncStatusState =
    | 'idle'
    | 'syncing'
    | 'ok'
    | 'err'
    | 'unreachable';

export type SyncReportDto = {
    totalCards: number;
    attempted: number;
    succeeded: number;
    failed: number;
    persistentFailures: string[];
};

export type AnkiSyncStatus = {
    state: AnkiSyncStatusState;
    lastFinishedAtMs?: number | null;
    lastError?: string | null;
    lastReport?: SyncReportDto | null;
};

/**
 * Map status state → Font Awesome icon. Returns null for `unreachable`,
 * which is the signal to hide the button entirely. Pure so it's
 * unit-testable without a render harness.
 */
export function iconForState(state: AnkiSyncStatusState): IconDefinition | null {
    switch (state) {
        case 'idle':
        case 'syncing':
            return faSync;
        case 'ok':
            return faCheck;
        case 'err':
            return faExclamationCircle;
        case 'unreachable':
            return null;
    }
}

/** True when the button should be rendered in the DOM at all. */
export function isVisible(state: AnkiSyncStatusState): boolean {
    return state !== 'unreachable';
}

/** True when the icon should spin (currently in progress). */
export function isSpinning(state: AnkiSyncStatusState): boolean {
    return state === 'syncing';
}

/** True when the click handler should be disabled (in flight). */
export function isClickDisabled(state: AnkiSyncStatusState): boolean {
    return state === 'syncing';
}

export async function triggerSyncNow(): Promise<SyncReportDto> {
    return await invoke<SyncReportDto>('sync_anki_now');
}
