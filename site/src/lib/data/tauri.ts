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
    let setter: ((value: T) => void) | null = null;
    let unsub: UnlistenFn | null = null;

    let getter = () => {
        invoke<T>(getterName, args).then((v) => {
            let setInitial = () => {
                if (setter) {
                    setter(v);
                } else {
                    setTimeout(setInitial, 10);
                };
            };
            setInitial();
        })
    };

    listen<TEvent>(updateEventName, (event) => {
        if (eventFilter(event.payload)) {
            getter();
        }
    }).then((u) => {
        unsub = u;
    });

    getter();

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