import { invoke, type InvokeArgs } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type UpdateEvent<TEvent = any> = {
    name: string;
    filter: (ev: TEvent) => boolean;
};

// Every event name the Rust backend emits that the frontend consumes through
// the hub (verified against `emit(` call sites under site/src-tauri/src).
// Native listeners for these are installed eagerly at hub construction, so an
// event fired while a view is still mounting — after its subscribe() but
// before an async native `listen()` registration would have completed — is
// not silently lost. The lyrics_* / spotify_* events are consumed via direct
// `listen()` in their stores and deliberately stay out of this catalog.
// Any name not listed here still gets a lazy native listener on first
// subscribe.
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
    // Exactly one of the two is set. Plain subscriptions hold their handler
    // strongly (alive until unsubscribed). Weak subscriptions hold only a
    // WeakRef, so the subscription lives exactly as long as the caller's own
    // strong reference to the handler; dispatch prunes dead entries.
    handler?: Handler;
    weakHandler?: WeakRef<Handler>;
};

// Singleton router: at most one Tauri `listen()` per event name, with
// in-process fan-out to subscribers. Each per-paragraph Resource subscribing
// here costs an O(1) Set insert instead of a ~10 ms IPC round-trip to
// register a native listener.
export class TauriEventHub {
    #subs = new Map<string, Set<Subscriber>>();
    #ready = new Map<string, Promise<void>>();
    #installed = new Set<string>();

    constructor(eagerEvents: readonly string[] = KNOWN_EVENTS) {
        // Best-effort: outside a Tauri webview (vitest, plain browser) the
        // native `listen` fails; each name then degrades to the lazy
        // install-on-first-subscribe path. Must never throw at import time.
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

    // Weakly-held subscription: the hub keeps only a WeakRef to `handler`,
    // so the caller MUST hold the only strong reference to it (see Resource).
    // Once the caller is collected, dispatch prunes the dead entry; the
    // returned unsubscribe removes it eagerly (e.g. from a finalizer).
    subscribeWeak<T>(
        name: string,
        filter: (payload: T) => boolean,
        handler: (payload: T) => void,
    ): () => void {
        const sub: Subscriber =
            typeof WeakRef === "undefined"
                ? // No WeakRef in this environment: fall back to the strong
                  // (pre-existing) behavior rather than dropping updates.
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
                // Native registration failed (e.g. no Tauri webview). Forget
                // the attempt so a later subscribe() retries lazily.
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
                // The weakly-held subscriber (a Resource) was collected;
                // drop its entry. Deleting during Set iteration is safe.
                set.delete(sub);
                continue;
            }
            try {
                if (sub.filter(payload)) handler(payload);
            } catch {
                /* swallow — one bad subscriber must not break others */
            }
        }
    }
}

export const eventHub = new TauriEventHub();

export class Resource<T> {
    #current: T | undefined = $state(undefined);
    // Sole strong reference to the refetch handler. The hub only holds a
    // WeakRef to it (subscribeWeak), so the subscription — and therefore the
    // backend re-fetch on each event — lives exactly as long as this
    // Resource. Dropped Resources become GC-eligible; dispatch prunes their
    // dead entries and the FinalizationRegistry unsubscribes them eagerly.
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

        // The held unsubscribers only capture the hub's Set and the
        // Subscriber entry (which references `fetch` weakly), so the registry
        // does not pin this Resource.
        eventCleanupRegistry?.register(this, unsubscribers);
    }

    get current(): T | undefined {
        return this.#current;
    }
}
