/**
 * Mock implementation of @tauri-apps/api/event for Playwright tests.
 */

export type EventCallback<T> = (event: Event<T>) => void;

export type Event<T> = {
  payload: T;
};

export type UnlistenFn = () => void;

// Shared event handlers map. Hoisted onto `globalThis` because Vite's
// optimizeDeps pre-bundles some Tauri plugins (e.g. `tauri-plugin-log`)
// with their own resolved copy of `@tauri-apps/api/event`, bypassing
// the alias for nested imports. Without sharing through globalThis,
// each consumer would see a separate handlers Map, and emits from one
// instance would never reach listeners registered in the other.
const eventHandlers: Map<string, Set<EventCallback<unknown>>> =
  ((globalThis as any).__tauriMockEventHandlers ??= new Map());

/**
 * Listen to an event from the backend.
 */
export async function listen<T>(
  event: string,
  handler: EventCallback<T>
): Promise<UnlistenFn> {
  if (!eventHandlers.has(event)) {
    eventHandlers.set(event, new Set());
  }

  const handlers = eventHandlers.get(event)!;
  handlers.add(handler as EventCallback<unknown>);

  // Return unlisten function
  return () => {
    handlers.delete(handler as EventCallback<unknown>);
  };
}

/**
 * Listen to an event from the backend (once).
 */
export async function once<T>(
  event: string,
  handler: EventCallback<T>
): Promise<UnlistenFn> {
  const unlisten = await listen<T>(event, (e) => {
    handler(e);
    unlisten();
  });
  return unlisten;
}

/**
 * Emits an event to all listeners.
 */
export function emit(event: string, payload?: unknown): void {
  const handlers = eventHandlers.get(event);
  if (handlers) {
    handlers.forEach(handler => handler({ payload }));
  }
}

/**
 * Emit an event to the backend (mock - does nothing in tests).
 */
export async function emitTo(
  target: string,
  event: string,
  payload?: unknown
): Promise<void> {
  console.log(`[Tauri Event Mock] emitTo: ${target}/${event}`, payload);
}

// Make emit available globally for internal use
if (typeof window !== 'undefined') {
  (window as any).__tauriEmit = emit;
}
