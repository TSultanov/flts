import { invoke, type InvokeArgs } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type UpdateEvent<TEvent = any> = {
    name: string;
    filter: (ev: TEvent) => boolean;
};

// Listened for eagerly at hub construction, so an event fired while a view is
// still mounting isn't lost to a pending async `listen()`. Unlisted names get
// a lazy listener on first subscribe; lyrics_*/spotify_* bypass the hub.
export const KNOWN_EVENTS: readonly string[] = [
    "paragraph_updated",
    "book_updated",
    "library_updated",
    "cards_updated",
    "paragraph_translation_started",
    "paragraph_translation_progress",
    "paragraph_translation_finished",
    "summary_generation_progress",
    "config_updated",
    "anki_sync_status_changed",
    "sync_status_changed",
];

const eventCleanupRegistry =
    typeof FinalizationRegistry !== "undefined"
        ? new FinalizationRegistry<Array<() => void>>((unsubscribers) => {
              for (const u of unsubscribers) {
                  try { u(); } catch { /* ignore */ }
              }
          })
        : null;

type Handler = (payload: any) => void;

type Subscriber = {
    filter: (payload: any) => boolean;
    // Exactly one is set. A weak subscription lives only as long as the
    // caller's own strong ref to the handler; dispatch prunes dead entries.
    handler?: Handler;
    weakHandler?: WeakRef<Handler>;
};

// At most one Tauri `listen()` per event name, fanned out in-process: a
// per-paragraph Resource costs a Set insert, not a ~10 ms IPC round-trip.
export class TauriEventHub {
    #subs = new Map<string, Set<Subscriber>>();
    #ready = new Map<string, Promise<void>>();
    #installed = new Set<string>();

    constructor(eagerEvents: readonly string[] = KNOWN_EVENTS) {
        // Outside a Tauri webview `listen` fails; degrade to lazy install
        // rather than throwing at import time.
        for (const name of eagerEvents) {
            try {
                this.#install(name);
            } catch {
                /* degrade to lazy */
            }
        }
    }

    subscribe<T>(
        name: string,
        filter: (payload: T) => boolean,
        handler: (payload: T) => void,
    ): () => void {
        return this.#add(name, {
            filter: filter as (p: any) => boolean,
            handler: handler as Handler,
        });
    }

    // The hub keeps only a WeakRef, so the caller MUST hold the only strong
    // reference to `handler` (see Resource).
    subscribeWeak<T>(
        name: string,
        filter: (payload: T) => boolean,
        handler: (payload: T) => void,
    ): () => void {
        const sub: Subscriber =
            typeof WeakRef === "undefined"
                ? // No WeakRef here: hold strongly rather than drop updates.
                  {
                      filter: filter as (p: any) => boolean,
                      handler: handler as Handler,
                  }
                : {
                      filter: filter as (p: any) => boolean,
                      weakHandler: new WeakRef(handler as Handler),
                  };
        return this.#add(name, sub);
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

    #add(name: string, sub: Subscriber): () => void {
        let set = this.#subs.get(name);
        if (!set) {
            set = new Set();
            this.#subs.set(name, set);
        }
        this.#install(name);
        set.add(sub);
        return () => set!.delete(sub);
    }

    #install(name: string) {
        if (this.#installed.has(name)) return;
        this.#installed.add(name);
        let p: Promise<void>;
        try {
            p = listen(name, (event) => {
                this.#dispatch(name, (event as any).payload);
            }).then(() => undefined);
        } catch (e) {
            p = Promise.reject(e);
        }
        this.#ready.set(
            name,
            p.catch(() => {
                // Forget the failed attempt so a later subscribe() retries.
                this.#installed.delete(name);
                this.#ready.delete(name);
            }),
        );
    }

    #dispatch(name: string, payload: any) {
        const set = this.#subs.get(name);
        if (!set || set.size === 0) return;
        for (const sub of set) {
            const handler = sub.handler ?? sub.weakHandler?.deref();
            if (!handler) {
                // Collected weak subscriber; deleting mid-iteration is safe.
                set.delete(sub);
                continue;
            }
            try {
                if (sub.filter(payload)) handler(payload);
            } catch {
                /* one bad subscriber must not break others */
            }
        }
    }
}

export const eventHub = new TauriEventHub();

export class Resource<T> {
    #current: T | undefined = $state(undefined);
    // Sole strong ref to the handler: the hub holds only a WeakRef, so the
    // subscription lives exactly as long as this Resource.
    #refetch: () => void;

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
        this.#refetch = fetch;

        for (const ev of events) {
            unsubscribers.push(eventHub.subscribeWeak(ev.name, ev.filter, fetch));
        }
        fetch();

        // The unsubscribers capture only the hub's Set and the (weak)
        // Subscriber, so the registry does not pin this Resource.
        eventCleanupRegistry?.register(this, unsubscribers);
    }

    get current(): T | undefined {
        return this.#current;
    }
}
