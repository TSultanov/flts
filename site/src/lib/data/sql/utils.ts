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