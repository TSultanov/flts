import type { Readable, Unsubscriber } from "svelte/store";

export const dbUpdatesChannelName = "db_updates_channel";

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
            if (data && count > 0) {
                setTimeout(() => {
                    unsubscriber();
                })
                resolve(data);
            }
            count++;
        }

        unsubscriber = store.subscribe(data => resolver(data));
    });
}