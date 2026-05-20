import { Resource } from '../data/tauri.svelte';
import type { AnkiSyncStatus } from './ankiSyncHelpers';

export const ankiSyncStatus = new Resource<AnkiSyncStatus>(
    'get_anki_sync_status',
    {},
    [{ name: 'anki_sync_status_changed', filter: () => true }],
    { state: 'unreachable' },
);
