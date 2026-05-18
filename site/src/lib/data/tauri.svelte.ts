import { invoke, type InvokeArgs } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { SvelteMap } from "svelte/reactivity";
import type { UUID } from "./v2/db";

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

export type ParagraphTranslationActivity = {
    requestId: number;
    progressChars: number;
    expectedChars: number;
};

type StartedEvent = {
    bookId: UUID;
    paragraphId: number;
    requestId: number;
    expectedChars: number;
};

type ProgressEvent = {
    bookId: UUID;
    paragraphId: number;
    requestId: number;
    progressChars: number;
    expectedChars: number;
};

type FinishedEvent = {
    bookId: UUID;
    paragraphId: number;
    requestId: number;
    error: string | null;
};

const activityKey = (bookId: UUID, paragraphId: number) =>
    `${bookId}:${paragraphId}`;

// Single source of truth for active paragraph translations. The Rust
// `TranslationQueue.active_translations` map starts empty at process boot
// and is purely in-memory, so subscribing once at module load is sufficient
// to capture every started/progress/finished event — no per-paragraph
// snapshot fetch is required.
class ActiveTranslationsStore {
    #entries = new SvelteMap<string, ParagraphTranslationActivity>();

    constructor() {
        eventHub.subscribe<StartedEvent>(
            "paragraph_translation_started",
            () => true,
            (p) => {
                this.#entries.set(activityKey(p.bookId, p.paragraphId), {
                    requestId: p.requestId,
                    progressChars: 0,
                    expectedChars: p.expectedChars,
                });
            },
        );
        eventHub.subscribe<ProgressEvent>(
            "paragraph_translation_progress",
            () => true,
            (p) => {
                this.#entries.set(activityKey(p.bookId, p.paragraphId), {
                    requestId: p.requestId,
                    progressChars: p.progressChars,
                    expectedChars: p.expectedChars,
                });
            },
        );
        eventHub.subscribe<FinishedEvent>(
            "paragraph_translation_finished",
            () => true,
            (p) => {
                if (p.error) {
                    console.warn(
                        `Translation failed for paragraph ${p.paragraphId}:`,
                        p.error,
                    );
                }
                this.#entries.delete(activityKey(p.bookId, p.paragraphId));
            },
        );
    }

    get(bookId: UUID, paragraphId: number): ParagraphTranslationActivity | null {
        return this.#entries.get(activityKey(bookId, paragraphId)) ?? null;
    }
}

export const activeTranslations = new ActiveTranslationsStore();

export class ParagraphTranslationActivityResource {
    #bookId!: UUID;
    #paragraphId!: number;

    current: ParagraphTranslationActivity | null = $derived.by(() =>
        activeTranslations.get(this.#bookId, this.#paragraphId),
    );

    constructor(bookId: UUID, paragraphId: number) {
        this.#bookId = bookId;
        this.#paragraphId = paragraphId;
    }
}
