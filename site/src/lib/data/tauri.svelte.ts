import { invoke, type InvokeArgs } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type UpdateEvent<TEvent = any> = {
    name: string;
    filter: (ev: TEvent) => boolean;
};

const eventCleanupRegistry =
    typeof FinalizationRegistry !== "undefined"
        ? new FinalizationRegistry<Array<() => void>>((unsubscribers) => {
              for (const u of unsubscribers) {
                  try { u(); } catch { /* ignore */ }
              }
          })
        : null;

type Subscriber = {
    filter: (payload: any) => boolean;
    handler: (payload: any) => void;
};

// Singleton router: at most one Tauri `listen()` per event name, with
// in-process fan-out to subscribers. Each per-paragraph Resource subscribing
// here costs an O(1) Set insert instead of a ~10 ms IPC round-trip to
// register a native listener.
class TauriEventHub {
    #subs = new Map<string, Set<Subscriber>>();
    #ready = new Map<string, Promise<void>>();

    subscribe<T>(
        name: string,
        filter: (payload: T) => boolean,
        handler: (payload: T) => void,
    ): () => void {
        let set = this.#subs.get(name);
        if (!set) {
            set = new Set();
            this.#subs.set(name, set);
            this.#install(name);
        }
        const sub: Subscriber = {
            filter: filter as (p: any) => boolean,
            handler: handler as (p: any) => void,
        };
        set.add(sub);
        return () => set!.delete(sub);
    }

    async subscribeReady<T>(
        name: string,
        filter: (payload: T) => boolean,
        handler: (payload: T) => void,
    ): Promise<() => void> {
        const unsub = this.subscribe(name, filter, handler);
        await this.#ready.get(name);
        return unsub;
    }

    #install(name: string) {
        const p = listen(name, (event) => {
            const set = this.#subs.get(name);
            if (!set || set.size === 0) return;
            const payload = (event as any).payload;
            for (const sub of set) {
                try {
                    if (sub.filter(payload)) sub.handler(payload);
                } catch {
                    /* swallow — one bad subscriber must not break others */
                }
            }
        }).then(() => undefined);
        this.#ready.set(name, p);
    }
}

export const eventHub = new TauriEventHub();

export class Resource<T> {
    #current: T | undefined = $state(undefined);

    constructor(
        getterName: string,
        args: InvokeArgs = {},
        events: UpdateEvent[] = [],
        defaultValue?: T,
    ) {
        this.#current = defaultValue;
        const unsubscribers: Array<() => void> = [];

        const fetch = () => {
            invoke<T>(getterName, args).then((v) => {
                this.#current = v;
            });
        };

        for (const ev of events) {
            unsubscribers.push(eventHub.subscribe(ev.name, ev.filter, fetch));
        }
        fetch();

        eventCleanupRegistry?.register(this, unsubscribers);
    }

    get current(): T | undefined {
        return this.#current;
    }
}
