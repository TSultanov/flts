import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { ChapterParagraphsStore } from "./ChapterParagraphsStore.svelte";
import type {
    Library,
    ParagraphOriginal,
    ParagraphSegment,
    ParagraphTranslationSlice,
} from "../data/library";
import type { UUID } from "../data/uuid";

// Test double for the eventHub singleton: records subscriptions, lets
// tests fire events, and tracks unsubscription via an `active` flag.
const hubState = vi.hoisted(() => {
    type Sub = {
        name: string;
        filter: (p: unknown) => boolean;
        handler: (p: unknown) => void;
        active: boolean;
    };
    const subs: Sub[] = [];
    return {
        subs,
        subscribe(
            name: string,
            filter: (p: unknown) => boolean,
            handler: (p: unknown) => void,
        ): () => void {
            const sub: Sub = { name, filter, handler, active: true };
            subs.push(sub);
            return () => {
                sub.active = false;
            };
        },
        fire(name: string, payload: unknown): void {
            for (const s of [...subs]) {
                if (s.active && s.name === name && s.filter(payload)) {
                    s.handler(payload);
                }
            }
        },
        reset(): void {
            subs.length = 0;
        },
    };
});

vi.mock("../data/tauri.svelte", () => ({ eventHub: hubState }));

type Deferred<T> = {
    promise: Promise<T>;
    resolve: (v: T) => void;
    reject: (e: unknown) => void;
};

function deferred<T>(): Deferred<T> {
    let resolve!: (v: T) => void;
    let reject!: (e: unknown) => void;
    const promise = new Promise<T>((res, rej) => {
        resolve = res;
        reject = rej;
    });
    return { promise, resolve, reject };
}

function makeLibraryStub() {
    const originalCalls: Array<{
        ids: number[];
        d: Deferred<ParagraphOriginal[]>;
    }> = [];
    const translationCalls: Array<{
        ids: number[];
        d: Deferred<ParagraphTranslationSlice[]>;
    }> = [];
    const library = {
        getParagraphOriginalsBatch: (_bookId: UUID, ids: number[]) => {
            const d = deferred<ParagraphOriginal[]>();
            originalCalls.push({ ids, d });
            return d.promise;
        },
        getParagraphTranslationsBatch: (_bookId: UUID, ids: number[]) => {
            const d = deferred<ParagraphTranslationSlice[]>();
            translationCalls.push({ ids, d });
            return d.promise;
        },
    } as unknown as Library;
    return { library, originalCalls, translationCalls };
}

const bookId = "test-book" as UUID;

const seg = (html: string): ParagraphSegment[] => [{ kind: "gap", html }];

const range = (start: number, end: number): number[] =>
    Array.from({ length: end - start }, (_, i) => start + i);

// Fake timers don't fake microtasks, so a few awaits drain the
// .then/.catch/.finally chains hanging off resolved deferreds.
async function flush(): Promise<void> {
    for (let i = 0; i < 10; i++) {
        await Promise.resolve();
    }
}

beforeEach(() => {
    hubState.reset();
    vi.useFakeTimers();
    vi.spyOn(console, "info").mockImplementation(() => {});
});

afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
});

describe("ChapterParagraphsStore.dispose", () => {
    it("unsubscribes both events, cancels the cards-refresh timer, and quiets the pumps", async () => {
        const { library, originalCalls, translationCalls } = makeLibraryStub();
        const store = new ChapterParagraphsStore(bookId, library);
        expect(hubState.subs.map((s) => s.name).sort()).toEqual([
            "cards_updated",
            "paragraph_updated",
        ]);

        // Cache one translation so a cards refresh would have work to do.
        store.enqueueTranslations([1]);
        translationCalls[0].d.resolve([{ id: 1, segments: seg("v1") }]);
        await flush();
        expect(store.getTranslation(1)?.segments).toEqual(seg("v1"));

        // Fill all original slots and leave one chunk queued to prove the
        // pump goes quiet after dispose.
        store.enqueueOriginals(range(0, 120));
        expect(originalCalls.length).toBe(5);

        // Schedule a cards refresh, then dispose before it fires.
        hubState.fire("cards_updated", null);
        store.dispose();
        expect(hubState.subs.every((s) => !s.active)).toBe(true);

        vi.advanceTimersByTime(1_000);
        await flush();
        expect(translationCalls.length).toBe(1); // canceled timer fetched nothing

        // Post-dispose events reach no handler and trigger no fetches.
        hubState.fire("paragraph_updated", { bookId, paragraphId: 1 });
        hubState.fire("cards_updated", null);
        vi.advanceTimersByTime(1_000);
        await flush();
        expect(translationCalls.length).toBe(1);

        // An in-flight resolution must not dispatch the queued chunk.
        originalCalls[0].d.resolve([{ id: 0, original: "o0" }]);
        await flush();
        expect(originalCalls.length).toBe(5);
    });
});

describe("ChapterParagraphsStore translation staleness", () => {
    it("drops the stale in-flight response and applies the refetch when paragraph_updated races a fetch", async () => {
        const { library, translationCalls } = makeLibraryStub();
        const store = new ChapterParagraphsStore(bookId, library);

        store.enqueueTranslations([1]);
        expect(translationCalls.length).toBe(1);

        // Update lands while the fetch is in flight → a refetch dispatches.
        hubState.fire("paragraph_updated", { bookId, paragraphId: 1 });
        expect(translationCalls.length).toBe(2);

        // The superseded response resolves; it must not be applied.
        translationCalls[0].d.resolve([{ id: 1, segments: seg("stale") }]);
        await flush();
        expect(store.getTranslation(1)).toBeNull();

        translationCalls[1].d.resolve([{ id: 1, segments: seg("fresh") }]);
        await flush();
        expect(store.getTranslation(1)?.segments).toEqual(seg("fresh"));
        store.dispose();
    });

    it("does not let an older overlapping batch overwrite newer data", async () => {
        const { library, translationCalls } = makeLibraryStub();
        const store = new ChapterParagraphsStore(bookId, library);

        store.enqueueTranslations([1]);
        hubState.fire("paragraph_updated", { bookId, paragraphId: 1 });
        expect(translationCalls.length).toBe(2);

        // Newer batch resolves first...
        translationCalls[1].d.resolve([{ id: 1, segments: seg("new") }]);
        await flush();
        expect(store.getTranslation(1)?.segments).toEqual(seg("new"));

        // ...then the older one arrives out of order and must be ignored.
        translationCalls[0].d.resolve([{ id: 1, segments: seg("old") }]);
        await flush();
        expect(store.getTranslation(1)?.segments).toEqual(seg("new"));
        store.dispose();
    });

    it("keeps cached segments visible during a refetch and never clobbers them with a null row", async () => {
        const { library, translationCalls } = makeLibraryStub();
        const store = new ChapterParagraphsStore(bookId, library);

        store.enqueueTranslations([1]);
        translationCalls[0].d.resolve([{ id: 1, segments: seg("v1") }]);
        await flush();
        expect(store.getTranslation(1)?.segments).toEqual(seg("v1"));

        hubState.fire("paragraph_updated", { bookId, paragraphId: 1 });
        // Old segments stay visible while the refetch is in flight.
        expect(store.getTranslation(1)?.segments).toEqual(seg("v1"));
        expect(translationCalls.length).toBe(2);

        // Backend transiently reports no segments; the cache keeps v1.
        translationCalls[1].d.resolve([{ id: 1 }]);
        await flush();
        expect(store.getTranslation(1)?.segments).toEqual(seg("v1"));
        store.dispose();
    });
});

describe("ChapterParagraphsStore batch watchdog", () => {
    it("releases hung slots after 30s, allows retries, and still applies a late response", async () => {
        const { library, originalCalls } = makeLibraryStub();
        const store = new ChapterParagraphsStore(bookId, library);

        store.enqueueOriginals(range(0, 100));
        expect(originalCalls.length).toBe(5); // all slots hung

        store.enqueueOriginals(range(100, 120));
        expect(originalCalls.length).toBe(5); // queued behind the wedge

        vi.advanceTimersByTime(30_000);
        // Slots released → the queued chunk dispatches.
        expect(originalCalls.length).toBe(6);
        expect(originalCalls[5].ids).toEqual(range(100, 120));

        // Timed-out ids left the dedup set, so re-enqueueing retries them.
        store.enqueueOriginals(range(0, 20));
        expect(originalCalls.length).toBe(7);
        expect(originalCalls[6].ids).toEqual(range(0, 20));

        // A late response after its timeout still applies its rows...
        originalCalls[0].d.resolve([{ id: 0, original: "late" }]);
        await flush();
        expect(store.getOriginal(0)).toBe("late");

        // ...but must not release a slot twice: with 2 batches in flight,
        // exactly 3 more of the 5 new chunks may dispatch.
        store.enqueueOriginals(range(200, 300));
        expect(originalCalls.length).toBe(10);
        store.dispose();
    });

    it("clears translation dedup on timeout so a future enqueue retries, and a late row obeys the null-clobber rule", async () => {
        const { library, translationCalls } = makeLibraryStub();
        const store = new ChapterParagraphsStore(bookId, library);

        store.enqueueTranslations([1]);
        expect(translationCalls.length).toBe(1);

        vi.advanceTimersByTime(30_000);
        store.enqueueTranslations([1]);
        expect(translationCalls.length).toBe(2);

        translationCalls[1].d.resolve([{ id: 1, segments: seg("v1") }]);
        await flush();
        expect(store.getTranslation(1)?.segments).toEqual(seg("v1"));

        // The hung batch finally resolves with a null row; its epoch is
        // unchanged so it reaches the write path, where the null-clobber
        // rule preserves v1.
        translationCalls[0].d.resolve([{ id: 1 }]);
        await flush();
        expect(store.getTranslation(1)?.segments).toEqual(seg("v1"));
        store.dispose();
    });
});

describe("post-watchdog invalidation", () => {
    it("honors paragraph_updated for a timed-out-but-pending fetch via in-flight tracking", async () => {
        const { library, translationCalls } = makeLibraryStub();
        const store = new ChapterParagraphsStore(bookId, library);

        // Paragraph 7 dispatched; the backend hangs past the watchdog.
        store.enqueueTranslations([7]);
        expect(translationCalls.length).toBe(1);
        vi.advanceTimersByTime(30_000);
        await flush();

        // The translation lands backend-side while the original fetch is
        // still pending; the update must be honored even though the id is
        // in neither the cache nor the dedup set anymore.
        hubState.fire("paragraph_updated", { bookId, paragraphId: 7 });
        expect(translationCalls.length).toBe(2);

        // The hung fetch finally resolves with a stale null row — it must
        // be epoch-rejected, not cached as untranslated.
        translationCalls[0].d.resolve([{ id: 7 }]);
        await flush();
        expect(store.getTranslation(7)).toBeNull();

        // The refetch resolves with the real segments.
        translationCalls[1].d.resolve([{ id: 7, segments: seg("v1") }]);
        await flush();
        expect(store.getTranslation(7)?.segments).toEqual(seg("v1"));
    });
});
