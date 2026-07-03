import { describe, it, expect, vi, afterEach } from 'vitest';

// Mock the native Tauri event layer. Callbacks are captured synchronously so
// tests can fire events "from the backend" at any point relative to the
// (asynchronously resolving) listen() promise.
const { listenMock, nativeListeners } = vi.hoisted(() => {
    const nativeListeners = new Map<
        string,
        Array<(event: { payload: unknown }) => void>
    >();
    const listenMock = vi.fn(
        (name: string, cb: (event: { payload: unknown }) => void) => {
            let arr = nativeListeners.get(name);
            if (!arr) {
                arr = [];
                nativeListeners.set(name, arr);
            }
            arr.push(cb);
            return Promise.resolve(() => {});
        },
    );
    return { listenMock, nativeListeners };
});

vi.mock('@tauri-apps/api/event', () => ({ listen: listenMock }));
vi.mock('@tauri-apps/api/core', () => ({
    invoke: vi.fn(() => Promise.resolve(undefined)),
}));

// Importing the module constructs the `eventHub` singleton, which must
// eagerly install native listeners for the whole known-event catalog.
import { eventHub, TauriEventHub, KNOWN_EVENTS } from './tauri.svelte';

/** Simulate the backend emitting an event to every registered native listener. */
function fireNative(name: string, payload: unknown): void {
    for (const cb of nativeListeners.get(name) ?? []) {
        cb({ payload });
    }
}

// WeakRef stand-in whose targets can be "collected" on demand, so the
// dispatch-time pruning path is exercised without relying on real GC.
class FakeWeakRef<T extends object> {
    static dead = new Set<object>();
    readonly #target: T;
    constructor(target: T) {
        this.#target = target;
    }
    deref(): T | undefined {
        return FakeWeakRef.dead.has(this.#target) ? undefined : this.#target;
    }
}

afterEach(() => {
    vi.unstubAllGlobals();
    FakeWeakRef.dead.clear();
});

describe('TauriEventHub eager catalog', () => {
    it('installs a native listener for every known event at construction', () => {
        // The singleton was constructed at module import; each catalog name
        // must already have a native listener registered.
        for (const name of KNOWN_EVENTS) {
            expect(
                listenMock,
                `expected eager listen("${name}")`,
            ).toHaveBeenCalledWith(name, expect.any(Function));
        }
    });

    it('delivers an event that arrives immediately after subscribe (no listen round-trip gap)', () => {
        // Because the native listener for a catalog event was installed at
        // hub construction, an event emitted right after subscribe() — e.g. a
        // background translation save landing while a chapter view mounts —
        // is delivered without awaiting any per-name listen() registration.
        const handler = vi.fn();
        const unsub = eventHub.subscribe<{ bookId: string }>(
            'paragraph_updated',
            (p) => p.bookId === 'b1',
            handler,
        );

        fireNative('paragraph_updated', { bookId: 'b1' });

        expect(handler).toHaveBeenCalledTimes(1);
        expect(handler).toHaveBeenCalledWith({ bookId: 'b1' });
        unsub();
    });

    it('still installs lazily for names outside the catalog', () => {
        const hub = new TauriEventHub([]);
        expect(nativeListeners.get('bespoke_event')).toBeUndefined();

        const handler = vi.fn();
        hub.subscribe('bespoke_event', () => true, handler);
        expect(listenMock).toHaveBeenCalledWith(
            'bespoke_event',
            expect.any(Function),
        );

        fireNative('bespoke_event', 42);
        expect(handler).toHaveBeenCalledWith(42);
    });
});

describe('plain (strong) subscribers', () => {
    it('unsubscribe stops delivery', () => {
        const hub = new TauriEventHub([]);
        const handler = vi.fn();
        const unsub = hub.subscribe('strong_evt', () => true, handler);

        fireNative('strong_evt', 'first');
        expect(handler).toHaveBeenCalledTimes(1);

        unsub();
        fireNative('strong_evt', 'second');
        expect(handler).toHaveBeenCalledTimes(1);
    });

    it('applies the filter before invoking the handler', () => {
        const hub = new TauriEventHub([]);
        const handler = vi.fn();
        const unsub = hub.subscribe<{ id: number }>(
            'filtered_evt',
            (p) => p.id === 7,
            handler,
        );

        fireNative('filtered_evt', { id: 3 });
        expect(handler).not.toHaveBeenCalled();

        fireNative('filtered_evt', { id: 7 });
        expect(handler).toHaveBeenCalledTimes(1);
        unsub();
    });
});

describe('weak subscribers', () => {
    it('delivers while the caller holds the handler, prunes once it is dead', () => {
        vi.stubGlobal('WeakRef', FakeWeakRef);
        const hub = new TauriEventHub([]);
        const handler = vi.fn();
        hub.subscribeWeak('weak_evt', () => true, handler);

        fireNative('weak_evt', 'alive');
        expect(handler).toHaveBeenCalledTimes(1);

        // Simulate the Resource (sole strong holder of the handler) being
        // garbage-collected.
        FakeWeakRef.dead.add(handler);
        fireNative('weak_evt', 'after-gc');
        expect(handler).toHaveBeenCalledTimes(1);

        // The dead entry must have been PRUNED by that dispatch, not merely
        // skipped: even if the ref were to deref again, the subscription is
        // gone from the hub.
        FakeWeakRef.dead.delete(handler);
        fireNative('weak_evt', 'after-prune');
        expect(handler).toHaveBeenCalledTimes(1);
    });

    it('pruning a dead weak entry does not disturb live subscribers of the same event', () => {
        vi.stubGlobal('WeakRef', FakeWeakRef);
        const hub = new TauriEventHub([]);
        const weakHandler = vi.fn();
        const strongHandler = vi.fn();
        hub.subscribeWeak('mixed_evt', () => true, weakHandler);
        hub.subscribe('mixed_evt', () => true, strongHandler);

        FakeWeakRef.dead.add(weakHandler);
        fireNative('mixed_evt', 'payload');

        expect(weakHandler).not.toHaveBeenCalled();
        expect(strongHandler).toHaveBeenCalledTimes(1);
    });

    it('unsubscribe removes a weak entry eagerly', () => {
        vi.stubGlobal('WeakRef', FakeWeakRef);
        const hub = new TauriEventHub([]);
        const handler = vi.fn();
        const unsub = hub.subscribeWeak('weak_unsub_evt', () => true, handler);

        unsub();
        fireNative('weak_unsub_evt', 'x');
        expect(handler).not.toHaveBeenCalled();
    });

    it('falls back to strong subscription when WeakRef is unavailable', () => {
        vi.stubGlobal('WeakRef', undefined);
        const hub = new TauriEventHub([]);
        const handler = vi.fn();
        hub.subscribeWeak('no_weakref_evt', () => true, handler);

        fireNative('no_weakref_evt', 'y');
        expect(handler).toHaveBeenCalledTimes(1);
    });
});
