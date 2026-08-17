import { describe, it, expect, beforeEach, vi } from 'vitest';
import type { UUID } from './uuid';

// Hoisted because the store subscribes at module load; `reset()` in
// beforeEach leaves only the current test's store wired up.
const hub = vi.hoisted(() => {
    type Handler = (payload: unknown) => void;
    type Filter = (payload: unknown) => boolean;
    const subs = new Map<string, Array<{ filter: Filter; handler: Handler }>>();
    return {
        subscribe(name: string, filter: Filter, handler: Handler) {
            let list = subs.get(name);
            if (!list) {
                list = [];
                subs.set(name, list);
            }
            list.push({ filter, handler });
            return () => {};
        },
        emit(name: string, payload: unknown) {
            for (const sub of subs.get(name) ?? []) {
                if (sub.filter(payload)) sub.handler(payload);
            }
        },
        reset() {
            subs.clear();
        },
    };
});

const invokeMock = vi.hoisted(() => vi.fn());

vi.mock('./tauri.svelte', () => ({
    eventHub: { subscribe: hub.subscribe },
}));
vi.mock('@tauri-apps/api/core', () => ({
    invoke: invokeMock,
}));

// One book id per test: stores from earlier imports keep their
// visibilitychange listeners on the shared jsdom document, and distinct
// books keep their state out of the current test's assertions.
const BOOK_A = 'aaaaaaaa-0000-0000-0000-000000000001' as UUID;
const BOOK_B = 'bbbbbbbb-0000-0000-0000-000000000002' as UUID;
const BOOK_C = 'cccccccc-0000-0000-0000-000000000003' as UUID;
const BOOK_D = 'dddddddd-0000-0000-0000-000000000004' as UUID;
const BOOK_E = 'eeeeeeee-0000-0000-0000-000000000005' as UUID;

type Store = typeof import('./translationActivity.svelte')['activeTranslations'];

const started = (bookId: UUID, paragraphId: number, requestId: number, expectedChars = 1000) =>
    hub.emit('paragraph_translation_started', { bookId, paragraphId, requestId, expectedChars });
const progress = (bookId: UUID, paragraphId: number, requestId: number, progressChars: number, expectedChars = 1000) =>
    hub.emit('paragraph_translation_progress', { bookId, paragraphId, requestId, progressChars, expectedChars });
const finished = (bookId: UUID, paragraphId: number, requestId: number, error: string | null = null) =>
    hub.emit('paragraph_translation_finished', { bookId, paragraphId, requestId, error });

const becomeVisible = () => {
    Object.defineProperty(document, 'visibilityState', {
        configurable: true,
        get: () => 'visible',
    });
    document.dispatchEvent(new Event('visibilitychange'));
};

let store: Store;

beforeEach(async () => {
    hub.reset();
    invokeMock.mockReset();
    // Snapshots report "nothing active" by default.
    invokeMock.mockResolvedValue([]);
    vi.resetModules();
    store = (await import('./translationActivity.svelte')).activeTranslations;
});

describe('activeTranslations lifecycle', () => {
    it('tracks started → progress → finished', () => {
        expect(store.get(BOOK_A, 1)).toBeNull();

        started(BOOK_A, 1, 10, 800);
        expect(store.get(BOOK_A, 1)).toEqual({
            requestId: 10,
            progressChars: 0,
            expectedChars: 800,
        });

        progress(BOOK_A, 1, 10, 300, 800);
        expect(store.get(BOOK_A, 1)).toEqual({
            requestId: 10,
            progressChars: 300,
            expectedChars: 800,
        });

        finished(BOOK_A, 1, 10);
        expect(store.get(BOOK_A, 1)).toBeNull();
    });

    it('does not resurrect an entry when progress arrives after finished', () => {
        started(BOOK_B, 2, 20);
        finished(BOOK_B, 2, 20);
        expect(store.get(BOOK_B, 2)).toBeNull();

        // The detached progress throttler can tick after finished.
        progress(BOOK_B, 2, 20, 950);
        expect(store.get(BOOK_B, 2)).toBeNull();
    });

    it('ignores progress whose requestId does not match the stored entry', () => {
        started(BOOK_C, 3, 30, 500);
        progress(BOOK_C, 3, 99, 400, 700); // stale/foreign request
        expect(store.get(BOOK_C, 3)).toEqual({
            requestId: 30,
            progressChars: 0,
            expectedChars: 500,
        });
    });
});

describe('reconciliation on visibilitychange', () => {
    it('replaces entries wholesale: deletes stale, updates tracked, adopts unknown', async () => {
        started(BOOK_D, 1, 40);
        started(BOOK_D, 2, 41);

        // Backgrounded: p1 finished, p2 progressed, p3 started — all events
        // lost, so only a snapshot can surface them.
        invokeMock.mockResolvedValue([
            { bookId: BOOK_D, paragraphId: 2, requestId: 41, progressChars: 640, expectedChars: 1000 },
            { bookId: BOOK_D, paragraphId: 3, requestId: 42, progressChars: 100, expectedChars: 900 },
        ]);

        becomeVisible();

        await vi.waitFor(() => {
            expect(store.get(BOOK_D, 1)).toBeNull();
            expect(store.get(BOOK_D, 2)).toEqual({
                requestId: 41,
                progressChars: 640,
                expectedChars: 1000,
            });
            expect(store.get(BOOK_D, 3)).toEqual({
                requestId: 42,
                progressChars: 100,
                expectedChars: 900,
            });
        });
        expect(invokeMock).toHaveBeenCalledWith('list_paragraph_translation_activity');
    });

    it('leaves entries untouched when the snapshot fetch fails', async () => {
        started(BOOK_A, 4, 44, 600);
        invokeMock.mockRejectedValue(new Error('ipc unavailable'));

        becomeVisible();
        await Promise.resolve();
        await Promise.resolve();
        await Promise.resolve();

        expect(store.get(BOOK_A, 4)).toEqual({
            requestId: 44,
            progressChars: 0,
            expectedChars: 600,
        });
    });

    it('discards a stale snapshot when a newer reconcile has started', async () => {
        started(BOOK_E, 5, 50);

        // Every live store reconciles per becomeVisible, in registration
        // order — so this test's resolver is the last of each batch.
        const pending: Array<(v: unknown) => void> = [];
        invokeMock.mockImplementation(
            () => new Promise((resolve) => pending.push(resolve)),
        );

        becomeVisible(); // reconcile #1 — snapshot left hanging
        await vi.waitFor(() => expect(pending.length).toBeGreaterThan(0));
        const batchSize = pending.length;
        becomeVisible(); // reconcile #2 supersedes #1
        await vi.waitFor(() => expect(pending.length).toBe(batchSize * 2));

        // #2 answers first: still active with fresh progress.
        pending[batchSize * 2 - 1]([
            { bookId: BOOK_E, paragraphId: 5, requestId: 50, progressChars: 720, expectedChars: 1000 },
        ]);
        await vi.waitFor(() => {
            expect(store.get(BOOK_E, 5)?.progressChars).toBe(720);
        });

        // #1's stale "nothing active" must not delete the live entry.
        pending[batchSize - 1]([]);
        await Promise.resolve();
        await Promise.resolve();
        expect(store.get(BOOK_E, 5)).toEqual({
            requestId: 50,
            progressChars: 720,
            expectedChars: 1000,
        });
    });
});

describe('lifecycle vs reconcile races', () => {
    it("does not delete a newer request's entry on a stale finished", () => {
        started(BOOK_B, 7, 60);
        finished(BOOK_B, 7, 59); // straggler from a previous request
        expect(store.get(BOOK_B, 7)).toEqual({
            requestId: 60,
            progressChars: 0,
            expectedChars: 1000,
        });
        finished(BOOK_B, 7, 60);
        expect(store.get(BOOK_B, 7)).toBeNull();
    });

    it('discards an in-flight reconcile when a lifecycle event lands during the pass', async () => {
        started(BOOK_C, 8, 70);

        const pending: Array<(v: unknown) => void> = [];
        invokeMock.mockImplementation(
            () => new Promise((resolve) => pending.push(resolve)),
        );
        becomeVisible();
        await vi.waitFor(() => expect(pending.length).toBeGreaterThan(0));
        const batchSize = pending.length;

        // The in-flight snapshot predates this finished and still holds the
        // entry; it must not resurrect the spinner.
        finished(BOOK_C, 8, 70);
        expect(store.get(BOOK_C, 8)).toBeNull();

        pending[batchSize - 1]([
            { bookId: BOOK_C, paragraphId: 8, requestId: 70, progressChars: 500, expectedChars: 1000 },
        ]);
        await Promise.resolve();
        await Promise.resolve();
        await Promise.resolve();
        expect(store.get(BOOK_C, 8)).toBeNull();
    });
});
