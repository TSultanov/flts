// WS transport to the Rust E2E bridge (src-tauri/src/bridge.rs), standing in
// for the webview IPC channel. Inbound frames are demuxed by SHAPE: `id` =>
// command reply, `event` => unsolicited broadcast.

type Pending = { resolve: (v: unknown) => void; reject: (e: unknown) => void };

export type BridgeEvent<T = unknown> = { event: string; payload: T };

let socket: WebSocket | null = null;
let ready: Promise<void> | null = null;
let nextId = 1;
const pending = new Map<number, Pending>();
const handlers = new Map<string, Set<(payload: unknown) => void>>();

function dispatch(raw: string): void {
  let frame: any;
  try {
    frame = JSON.parse(raw);
  } catch {
    return;
  }
  if (frame == null || typeof frame !== 'object') return;
  if (frame.id !== undefined && frame.id !== null) {
    const p = pending.get(frame.id);
    if (!p) return;
    pending.delete(frame.id);
    if ('err' in frame) p.reject(frame.err);
    else p.resolve(frame.ok);
  } else if (typeof frame.event === 'string') {
    for (const h of [...(handlers.get(frame.event) ?? [])]) h(frame.payload);
  }
}

function failAllPending(reason: unknown): void {
  for (const [, p] of [...pending]) p.reject(reason);
  pending.clear();
}

function ensureConnected(): Promise<void> {
  if (ready) return ready;
  ready = new Promise<void>((resolve, reject) => {
    const port = (globalThis as any).__FLTS_BRIDGE_PORT;
    if (!port) {
      reject(new Error('bridge port not injected (__FLTS_BRIDGE_PORT)'));
      return;
    }
    const ws = new WebSocket(`ws://127.0.0.1:${port}/bridge`);
    socket = ws;
    ws.onopen = () => resolve();
    ws.onerror = () => reject(new Error('bridge socket error'));
    ws.onclose = () => {
      // Callers hang forever otherwise; a dropped bridge is unrecoverable.
      failAllPending(new Error('bridge socket closed'));
      if (socket === ws) {
        socket = null;
        ready = null;
      }
    };
    ws.onmessage = (msg: MessageEvent) => dispatch(String(msg.data));
  });
  // A failed connect must not be cached as a permanently poisoned attempt.
  ready.catch(() => {
    ready = null;
    socket = null;
  });
  return ready;
}

export async function bridgeInvoke<T>(
  cmd: string,
  args: Record<string, unknown> = {},
): Promise<T> {
  await ensureConnected();
  return new Promise<T>((resolve, reject) => {
    const id = nextId++;
    pending.set(id, { resolve: resolve as (v: unknown) => void, reject });
    try {
      socket!.send(JSON.stringify({ id, cmd, args }));
    } catch (err) {
      pending.delete(id);
      reject(err);
    }
  });
}

export async function bridgeListen<T>(
  event: string,
  handler: (e: BridgeEvent<T>) => void,
): Promise<() => void> {
  await ensureConnected();
  let set = handlers.get(event);
  if (!set) handlers.set(event, (set = new Set()));
  const wrapped = (payload: unknown) =>
    handler({ event, payload: payload as T });
  set.add(wrapped);
  return () => {
    set!.delete(wrapped);
  };
}

// Escape hatch for Playwright page.evaluate in the real-backend tier.
if (typeof window !== 'undefined') {
  (window as any).__bridgeDebugInvoke = bridgeInvoke;
}
