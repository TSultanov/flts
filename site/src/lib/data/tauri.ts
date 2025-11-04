import { invoke, type InvokeArgs } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { readable, type Readable } from "svelte/store";

export function eventToReadable<T>(eventName: string, getterName: string): Readable<T | undefined>;
export function eventToReadable<T>(eventName: string, getterName: string, defaultValue: T): Readable<T>;
export function eventToReadable<T>(eventName: string, getterName: string, defaultValue: T | undefined = undefined): Readable<T | undefined> {
    let setter: ((value: T) => void) | null = null;
    let unsub: UnlistenFn | null = null;
    listen<T>(eventName, (event) => {
        if (setter) {
            setter(event.payload);
        }
    }).then((u) => {
        unsub = u;
    });

    invoke<T>(getterName).then((v) => {
        let setInitial = () => {
            if (setter) {
                setter(v);
            } else {
                setTimeout(setInitial, 10);
            };
        };
        setInitial();
    })

    return readable<T>(defaultValue, (set) => {
        setter = set;
        return () => {
            let doUnsub = () => {
                if (unsub) {
                    unsub();
                } else {
                    setTimeout(doUnsub, 10);
                }
            };
            doUnsub();
        };
    });
}

export function getterToReadable<T, TEvent>(getterName: string, args: InvokeArgs, updateEventName: string, eventFilter: (ev: TEvent) => boolean): Readable<T | undefined>
export function getterToReadable<T, TEvent>(getterName: string, args: InvokeArgs, updateEventName: string, eventFilter: (ev: TEvent) => boolean, defaultValue: T): Readable<T>
export function getterToReadable<T, TEvent>(getterName: string, args: InvokeArgs, updateEventName: string, eventFilter: (ev: TEvent) => boolean, defaultValue: T | undefined = undefined): Readable<T | undefined> {
    // Delegate to the multi-event version with a single event descriptor
    const events = [{ name: updateEventName, filter: (ev: unknown) => eventFilter(ev as TEvent) }];
    return getterToReadableWithEvents<T>(getterName, args, events, defaultValue as any) as Readable<T | undefined>;
}

// Listen to multiple events and refresh the getter when any filter matches
export type UpdateEvent<TEvent = any> = {
    name: string,
    filter: (ev: TEvent) => boolean,
};

export function getterToReadableWithEvents<T>(
    getterName: string,
    args: InvokeArgs,
    events: UpdateEvent[],
): Readable<T | undefined>;
export function getterToReadableWithEvents<T>(
    getterName: string,
    args: InvokeArgs,
    events: UpdateEvent[],
    defaultValue: T,
): Readable<T>;
export function getterToReadableWithEvents<T>(
    getterName: string,
    args: InvokeArgs,
    events: UpdateEvent[],
    defaultValue: T | undefined = undefined,
): Readable<T | undefined> {
    let setter: ((value: T) => void) | null = null;
    const unsubs: UnlistenFn[] = [];

    const getter = () => {
        invoke<T>(getterName, args).then((v) => {
            const setInitial = () => {
                if (setter) {
                    setter(v);
                } else {
                    setTimeout(setInitial, 10);
                }
            };
            setInitial();
        });
    };

    for (const ev of events) {
        listen(ev.name, (event) => {
            if (ev.filter((event as any).payload)) {
                getter();
            }
        }).then((u) => unsubs.push(u));
    }

    getter();

    return readable<T>(defaultValue, (set) => {
        setter = set;
        return () => {
            for (const u of unsubs) {
                try { u(); } catch { }
            }
        };
    });
}