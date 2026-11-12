import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { ChapterParagraphsStore } from "./ChapterParagraphsStore.svelte";
import type {
  Library,
  ParagraphOriginal,
  ParagraphSegment,
  ParagraphTranslationSlice,
} from "../data/library";
import type { UUID } from "../data/uuid";

// eventHub double: records subscriptions, fires events, tracks unsubscribe.
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

const seg = (text: string): ParagraphSegment[] => [{ kind: "gap", text }];

const range = (start: number, end: number): number[] =>
  Array.from({ length: end - start }, (_, i) => start + i);

// Fake timers don't fake microtasks; drain the chains off resolved deferreds.
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

    // Leave a chunk queued to prove the pump goes quiet after dispose.
    store.enqueueTranslations(range(100, 220));
    expect(translationCalls.length).toBe(6);

    // Schedule a cards refresh, then dispose before it fires.
    hubState.fire("cards_updated", null);
    store.dispose();
    expect(hubState.subs.every((s) => !s.active)).toBe(true);

    vi.advanceTimersByTime(1_000);
    await flush();
    expect(translationCalls.length).toBe(6); // canceled timer fetched nothing

    // Post-dispose events reach no handler and trigger no fetches.
    hubState.fire("paragraph_updated", { bookId, paragraphId: 1 });
    hubState.fire("cards_updated", null);
    vi.advanceTimersByTime(1_000);
    await flush();
    expect(translationCalls.length).toBe(6);

    // An in-flight resolution must not dispatch the queued chunk.
    translationCalls[1].d.resolve([]);
    await flush();
    expect(translationCalls.length).toBe(6);
  });
});

describe("ChapterParagraphsStore originals", () => {
  it("asks for everything in one request and never twice", async () => {
    const { library, originalCalls } = makeLibraryStub();
    const store = new ChapterParagraphsStore(bookId, library);

    store.fetchOriginals(range(0, 500));
    expect(originalCalls.map((c) => c.ids)).toEqual([range(0, 500)]);

    store.fetchOriginals(range(400, 600));
    expect(originalCalls[1].ids).toEqual(range(500, 600));
    store.fetchOriginals(range(0, 600));
    expect(originalCalls.length).toBe(2);

    expect(store.hasOriginal(3)).toBe(false);
    originalCalls[0].d.resolve([{ id: 3, original: "three" }]);
    await flush();
    expect(store.getOriginal(3)).toBe("three");
    store.dispose();
  });

  it("can be asked again for what a failed request did not bring", async () => {
    const { library, originalCalls } = makeLibraryStub();
    vi.spyOn(console, "error").mockImplementation(() => {});
    const store = new ChapterParagraphsStore(bookId, library);

    store.fetchOriginals(range(0, 5));
    originalCalls[0].d.reject(new Error("gone"));
    await flush();

    store.fetchOriginals(range(0, 5));
    expect(originalCalls[1].ids).toEqual(range(0, 5));
    store.dispose();
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
  it("releases hung slots after 30s and dispatches what queued behind them", async () => {
    const { library, translationCalls } = makeLibraryStub();
    const store = new ChapterParagraphsStore(bookId, library);

    store.enqueueTranslations(range(0, 120));
    expect(translationCalls.length).toBe(5); // all slots hung

    vi.advanceTimersByTime(30_000);
    expect(translationCalls.length).toBe(6);
    expect(translationCalls[5].ids).toEqual(range(100, 120));

    // A late response frees no slot twice: 1 in flight, so 4 more dispatch.
    translationCalls[0].d.resolve([]);
    await flush();
    store.enqueueTranslations(range(200, 400));
    expect(translationCalls.length).toBe(10);
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

    // The hung batch's null row has an unchanged epoch, so it reaches
    // the write path — where the null-clobber rule preserves v1.
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

    // The update must be honored even though the id is in neither the
    // cache nor the dedup set.
    hubState.fire("paragraph_updated", { bookId, paragraphId: 7 });
    expect(translationCalls.length).toBe(2);

    // The stale null row must be epoch-rejected, not cached.
    translationCalls[0].d.resolve([{ id: 7 }]);
    await flush();
    expect(store.getTranslation(7)).toBeNull();

    // The refetch resolves with the real segments.
    translationCalls[1].d.resolve([{ id: 7, segments: seg("v1") }]);
    await flush();
    expect(store.getTranslation(7)?.segments).toEqual(seg("v1"));
  });
});
