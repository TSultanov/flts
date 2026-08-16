// Real-backend stand-in for @tauri-apps/api/event: subscriptions are fed by
// bridge event frames. Export surface mirrors tests/mocks/tauri-event.ts.
import { bridgeListen } from './bridge-transport';

export type Event<T> = { event: string; id: number; payload: T };

export type EventCallback<T> = (event: Event<T>) => void;

export type UnlistenFn = () => void;

let nextEventId = 1;

export async function listen<T>(
  event: string,
  handler: EventCallback<T>,
): Promise<UnlistenFn> {
  return bridgeListen<T>(event, (e) =>
    handler({ event: e.event, id: nextEventId++, payload: e.payload }),
  );
}

export async function once<T>(
  event: string,
  handler: EventCallback<T>,
): Promise<UnlistenFn> {
  let unlisten: UnlistenFn = () => {};
  let fired = false;
  unlisten = await bridgeListen<T>(event, (e) => {
    if (fired) return;
    fired = true;
    unlisten();
    handler({ event: e.event, id: nextEventId++, payload: e.payload });
  });
  return unlisten;
}

// The backend is real here — there is no frontend→backend event channel on the
// bridge, so emits are inert (kept for export-surface parity).
export async function emit(event: string, payload?: unknown): Promise<void> {
  console.log(`[Bridge Shim] emit ignored: ${event}`, payload);
}

export async function emitTo(
  target: string,
  event: string,
  payload?: unknown,
): Promise<void> {
  console.log(`[Bridge Shim] emitTo ignored: ${target}/${event}`, payload);
}
