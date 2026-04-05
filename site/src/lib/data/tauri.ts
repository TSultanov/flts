import { invoke, type InvokeArgs } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { readable, type Readable } from "svelte/store";

// All backend events are wrapped in VersionedPayload { version, data }.
// Helpers below unwrap transparently so callers never see the wrapper.
type VersionedPayload<T> = { version: number; data: T };

function unwrapVersioned<T>(payload: unknown): { version: number; data: T } | null {
    if (
        payload !== null &&
        typeof payload === "object" &&
        "version" in payload &&
        "data" in payload
    ) {
        return payload as VersionedPayload<T>;
    }
    return null;
}

export function eventToReadable<T>(eventName: string, getterName: string): Readable<T | undefined>;
export function eventToReadable<T>(eventName: string, getterName: string, defaultValue: T): Readable<T>;
export function eventToReadable<T>(eventName: string, getterName: string, defaultValue: T | undefined = undefined): Readable<T | undefined> {
    let setter: ((value: T) => void) | null = null;
    let unsub: UnlistenFn | null = null;
    let lastVersion = 0;
    listen<VersionedPayload<T>>(eventName, (event) => {
        if (setter) {
            const v = unwrapVersioned<T>(event.payload);
            if (v && v.version > lastVersion) {
                lastVersion = v.version;
                setter(v.data);
            }
        }
    }).then((u) => {
        unsub = u;
    });

    invoke<T>(getterName).then((v) => {
        let setInitial = () => {
            if (setter) {
                // Only apply invoke result if no versioned event has arrived yet
                if (lastVersion === 0) {
                    setter(v);
                }
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
    let fetchGeneration = 0;

    const getter = () => {
        const gen = ++fetchGeneration;
        invoke<T>(getterName, args).then((v) => {
            if (gen !== fetchGeneration) return; // stale response
            const setInitial = () => {
                if (setter) {
                    if (gen !== fetchGeneration) return; // re-check after delay
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
            const vp = unwrapVersioned((event as any).payload);
            const data = vp ? vp.data : (event as any).payload;
            if (ev.filter(data)) {
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
    let fetchGeneration = 0;
    let lastPatchVersion = 0;

    const setValue = (value: T) => {
        current = value;
        if (setter) {
            setter(value);
        }
    };

    const getter = (triggerVersion: number) => {
        const gen = ++fetchGeneration;
        invoke<T>(getterName, args).then((v) => {
            if (gen !== fetchGeneration) return; // stale response
            // The invoke result is at least as fresh as the trigger event
            lastPatchVersion = Math.max(lastPatchVersion, triggerVersion);
            setValue(v);
        });
    };

    for (const ev of refreshEvents) {
        unsubPromises.push(
            listen(ev.name, (event) => {
                const vp = unwrapVersioned((event as any).payload);
                const data = vp ? vp.data : (event as any).payload;
                const version = vp ? vp.version : 0;
                if (ev.filter(data)) {
                    getter(version);
                }
            }),
        );
    }

    for (const ev of patchEvents) {
        unsubPromises.push(
            listen(ev.name, (event) => {
                const vp = unwrapVersioned((event as any).payload);
                const data = vp ? vp.data : (event as any).payload;
                const version = vp ? vp.version : 0;
                if (version > 0 && version <= lastPatchVersion) {
                    return; // stale patch
                }
                if (!ev.filter(data)) {
                    return;
                }
                if (current === undefined) {
                    return;
                }
                if (version > lastPatchVersion) {
                    lastPatchVersion = version;
                }
                const next = ev.patch(current, data);
                if (next === current) {
                    return;
                }
                setValue(next);
            }),
        );
    }

    getter(0);

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
