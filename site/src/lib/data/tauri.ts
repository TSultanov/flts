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

export function getterToReadable<T>(getterName: string, args: InvokeArgs, defaultValue: T): Readable<T>
export function getterToReadable<T>(getterName: string, args: InvokeArgs, defaultValue: T | undefined): Readable<T | undefined> {
    let setter: ((value: T) => void) | null = null;
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

    return readable<T>(defaultValue, (set) => {
        setter = set;
        return () => {};
    });
}