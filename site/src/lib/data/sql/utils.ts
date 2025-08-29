import type { Readable, Unsubscriber } from "svelte/store";

export const DB_UPDATES_CHANNEL_NAME = "db_updates_channel";

// Centralized SQLite OPFS DB naming
export const DB_FILE_NAME = 'db3.sqlite3';
export const DB_FILE_PATH = `/${DB_FILE_NAME}`;

export function debounce(callbackFn: () => void | Promise<void>, timeout: number) {
    let timeoutId: NodeJS.Timeout | null = null;
    let lastCallTime = 0;

    return () => {
        const now = Date.now();
        if (now - lastCallTime >= timeout) {
            lastCallTime = now;
            callbackFn();
            if (timeoutId) {
                clearTimeout(timeoutId);
                timeoutId = null;
            }
            return;
        }

        if (timeoutId) {
            clearTimeout(timeoutId);
            timeoutId = null;
        }

        timeoutId = setTimeout(() => {
            lastCallTime = Date.now(),
                callbackFn();
            timeoutId = null;
        }, timeout);
    }
}

export function readableToPromise<T>(store: Readable<T>): Promise<T | undefined> {
    return new Promise<T | undefined>(resolve => {
        let unsubscriber: Unsubscriber;
        let count = 0;

        const resolver = (data: T | undefined) => {
            if (count == 0) {
                count++;
                return; // Discard first bogus result
            }
            setTimeout(() => {
                unsubscriber();
            })
            resolve(data);
        }

        unsubscriber = store.subscribe(data => resolver(data));
    });
}

// UUID <-> 16-byte BLOB helpers for SQLite
// Store UUIDs as compact 16-byte blobs in SQLite and convert to strings in app code.
export function uuidToBlob(u: string): Uint8Array {
    const hex = u.replace(/-/g, '').toLowerCase();
    if (hex.length !== 32) throw new Error(`uuidToBlob: invalid UUID string: ${u}`);
    const out = new Uint8Array(16);
    for (let i = 0; i < 16; i++) {
        const byte = hex.slice(i * 2, i * 2 + 2);
        const v = parseInt(byte, 16);
        if (Number.isNaN(v)) throw new Error(`uuidToBlob: invalid hex in UUID: ${u}`);
        out[i] = v;
    }
    return out;
}

export function blobToUuid(b: Uint8Array | ArrayBuffer | number[]): string {
    const bytes = b instanceof Uint8Array ? b : b instanceof ArrayBuffer ? new Uint8Array(b) : new Uint8Array(b);
    if (bytes.length !== 16) throw new Error(`blobToUuid: expected 16 bytes, got ${bytes.length}`);
    const hex = Array.from(bytes, (v) => v.toString(16).padStart(2, '0')).join('');
    const s = `${hex.substring(0, 8)}-${hex.substring(8, 12)}-${hex.substring(12, 16)}-${hex.substring(16, 20)}-${hex.substring(20)}`;
    return s;
}