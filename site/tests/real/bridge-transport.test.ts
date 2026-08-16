// @vitest-environment node
// jsdom has no WebSocket; `ws` provides both server and client here.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { WebSocket, WebSocketServer, type WebSocket as WsSocket } from 'ws';

type ServerFrame = Record<string, unknown>;

let server: WebSocketServer;
let sockets: WsSocket[] = [];
let onFrame: (frame: any, send: (f: ServerFrame) => void) => void;

async function startServer(): Promise<number> {
  server = new WebSocketServer({ host: '127.0.0.1', port: 0 });
  await new Promise<void>((resolve) => server.once('listening', resolve));
  server.on('connection', (sock) => {
    sockets.push(sock);
    const send = (f: ServerFrame) => sock.send(JSON.stringify(f));
    sock.on('message', (data) => onFrame(JSON.parse(String(data)), send));
  });
  return (server.address() as { port: number }).port;
}

/** Fresh module instance — the transport keeps a module-level socket. */
async function loadTransport() {
  vi.resetModules();
  return import('./bridge-transport');
}

beforeEach(async () => {
  (globalThis as any).WebSocket = WebSocket;
  (globalThis as any).window = globalThis;
  onFrame = (frame, send) => send({ id: frame.id, ok: { echo: frame.args } });
  (globalThis as any).__FLTS_BRIDGE_PORT = await startServer();
});

afterEach(async () => {
  for (const s of sockets) s.terminate();
  sockets = [];
  await new Promise<void>((resolve) => server.close(() => resolve()));
});

describe('bridgeInvoke', () => {
  it('resolves with the ok payload', async () => {
    const { bridgeInvoke } = await loadTransport();
    await expect(bridgeInvoke('list_books', { a: 1 })).resolves.toEqual({
      echo: { a: 1 },
    });
  });

  it('rejects with the err value verbatim', async () => {
    onFrame = (frame, send) => send({ id: frame.id, err: 'book not found' });
    const { bridgeInvoke } = await loadTransport();
    await expect(bridgeInvoke('get_paragraph_view')).rejects.toBe(
      'book not found',
    );
  });

  it('routes concurrent replies to the right callers', async () => {
    // Reply out of order to prove id-keyed demux, not FIFO.
    const queued: Array<{ frame: any; send: (f: ServerFrame) => void }> = [];
    onFrame = (frame, send) => {
      queued.push({ frame, send });
      if (queued.length < 3) return;
      for (const q of queued.reverse()) {
        q.send({ id: q.frame.id, ok: q.frame.args.n });
      }
    };
    const { bridgeInvoke } = await loadTransport();
    const results = await Promise.all([
      bridgeInvoke('c', { n: 'first' }),
      bridgeInvoke('c', { n: 'second' }),
      bridgeInvoke('c', { n: 'third' }),
    ]);
    expect(results).toEqual(['first', 'second', 'third']);
  });

  it('ignores event frames while a reply is outstanding', async () => {
    onFrame = (frame, send) => {
      send({ event: 'book_updated', payload: 'noise' });
      send({ id: frame.id, ok: 'done' });
    };
    const { bridgeInvoke } = await loadTransport();
    await expect(bridgeInvoke('c')).resolves.toBe('done');
  });

  it('rejects when no bridge port was injected', async () => {
    delete (globalThis as any).__FLTS_BRIDGE_PORT;
    const { bridgeInvoke } = await loadTransport();
    await expect(bridgeInvoke('c')).rejects.toThrow(/bridge port/);
  });
});

describe('bridgeListen', () => {
  it('delivers event frames to listeners and stops after unlisten', async () => {
    onFrame = (frame, send) => {
      send({ event: 'book_updated', payload: { id: frame.args.id } });
      send({ id: frame.id, ok: null });
    };
    const { bridgeInvoke, bridgeListen } = await loadTransport();
    const seen: unknown[] = [];
    const unlisten = await bridgeListen('book_updated', (e) => {
      expect(e.event).toBe('book_updated');
      seen.push(e.payload);
    });

    await bridgeInvoke('c', { id: 1 });
    await vi.waitFor(() => expect(seen).toEqual([{ id: 1 }]));

    unlisten();
    await bridgeInvoke('c', { id: 2 });
    await new Promise((r) => setTimeout(r, 20));
    expect(seen).toEqual([{ id: 1 }]);
  });

  it('keeps other listeners alive when one unlistens', async () => {
    onFrame = (frame, send) => {
      send({ event: 'cards_updated', payload: null });
      send({ id: frame.id, ok: null });
    };
    const { bridgeInvoke, bridgeListen } = await loadTransport();
    let a = 0;
    let b = 0;
    const unA = await bridgeListen('cards_updated', () => a++);
    await bridgeListen('cards_updated', () => b++);
    unA();
    await bridgeInvoke('c');
    await vi.waitFor(() => expect(b).toBe(1));
    expect(a).toBe(0);
  });
});
