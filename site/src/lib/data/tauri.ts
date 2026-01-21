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

export type PatchEvent<T, TEvent = any> = {
    name: string,
    filter: (ev: TEvent) => boolean,
    patch: (current: T, ev: TEvent) => T,
};

export function getterToReadableWithEvents<T>(
    getterName: string,
): Readable<T | undefined>;
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
    args: InvokeArgs = {},
    events: UpdateEvent[] = [],
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

export function getterToReadableWithEventsAndPatches<T>(
    getterName: string,
): Readable<T | undefined>;
export function getterToReadableWithEventsAndPatches<T>(
    getterName: string,
    args: InvokeArgs,
    refreshEvents: UpdateEvent[],
    patchEvents: PatchEvent<T>[],
): Readable<T | undefined>;
export function getterToReadableWithEventsAndPatches<T>(
    getterName: string,
    args: InvokeArgs,
    refreshEvents: UpdateEvent[],
    patchEvents: PatchEvent<T>[],
    defaultValue: T,
): Readable<T>;
export function getterToReadableWithEventsAndPatches<T>(
    getterName: string,
    args: InvokeArgs = {},
    refreshEvents: UpdateEvent[] = [],
    patchEvents: PatchEvent<T>[] = [],
    defaultValue: T | undefined = undefined,
): Readable<T | undefined> {
    let setter: ((value: T) => void) | null = null;
    let current = defaultValue;
    const unsubPromises: Promise<UnlistenFn>[] = [];

    const setValue = (value: T) => {
        current = value;
        if (setter) {
            setter(value);
        }
    };

    const getter = () => {
        invoke<T>(getterName, args).then((v) => {
            setValue(v);
        });
    };

    for (const ev of refreshEvents) {
        unsubPromises.push(
            listen(ev.name, (event) => {
                if (ev.filter((event as any).payload)) {
                    getter();
                }
            }),
        );
    }

    for (const ev of patchEvents) {
        unsubPromises.push(
            listen(ev.name, (event) => {
                const payload = (event as any).payload;
                if (!ev.filter(payload)) {
                    return;
                }
                if (current === undefined) {
                    return;
                }
                const next = ev.patch(current, payload);
                if (next === current) {
                    return;
                }
                setValue(next);
            }),
        );
    }

    getter();

    return readable<T>(defaultValue as any, (set) => {
        setter = set;
        if (current !== undefined) {
            set(current as any);
        }
        return () => {
            for (const p of unsubPromises) {
                p.then((u) => u()).catch(() => { });
            }
        };
    });
}
