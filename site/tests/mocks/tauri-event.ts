/** Stand-in for @tauri-apps/api/event in Playwright tests. */

export type EventCallback<T> = (event: Event<T>) => void;

export type Event<T> = {
  payload: T;
};

export type UnlistenFn = () => void;

// On `globalThis` because Vite's optimizeDeps gives some plugins their own
// copy of this module; separate handler Maps would never see each other's emits.
const eventHandlers: Map<string, Set<EventCallback<unknown>>> =
  ((globalThis as any).__tauriMockEventHandlers ??= new Map());

export async function listen<T>(
  event: string,
  handler: EventCallback<T>
): Promise<UnlistenFn> {
  if (!eventHandlers.has(event)) {
    eventHandlers.set(event, new Set());
  }

  const handlers = eventHandlers.get(event)!;
  handlers.add(handler as EventCallback<unknown>);

  return () => {
    handlers.delete(handler as EventCallback<unknown>);
  };
}

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

export function emit(event: string, payload?: unknown): void {
  const handlers = eventHandlers.get(event);
  if (handlers) {
    handlers.forEach(handler => handler({ payload }));
  }
}

export async function emitTo(
  target: string,
  event: string,
  payload?: unknown
): Promise<void> {
  console.log(`[Tauri Event Mock] emitTo: ${target}/${event}`, payload);
}

if (typeof window !== 'undefined') {
  (window as any).__tauriEmit = emit;
}
