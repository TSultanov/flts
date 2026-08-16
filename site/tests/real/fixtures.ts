import { test as base, expect } from '@playwright/test';
import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { WebSocket } from 'ws';
import { setHarness } from './harness-registry';
import { SimClient } from './sim-client';

export { expect };
export type { SimRule, SimRequest } from './sim-client';

const repoRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  '../../..',
);

export type RealHarness = {
  llm: SimClient;
  lrclib: SimClient;
  anki: SimClient;
  bridgePort: number;
  configDir: string;
  appStderr: () => string;
  /** Direct bridge invoke from Node (no page needed). */
  invoke: <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>;
};

type SimPorts = { llm: number; lrclib: number; anki: number };

/** Resolves on the first stdout line matching `match`; rejects on timeout/exit. */
function awaitStdoutLine(
  child: ChildProcessWithoutNullStreams,
  what: string,
  match: (line: string) => boolean,
  timeoutMs: number,
  onOtherLine?: (line: string) => void,
): Promise<string> {
  return new Promise((resolve, reject) => {
    let buf = '';
    let done = false;
    const finish = (fn: () => void) => {
      if (done) return;
      done = true;
      clearTimeout(timer);
      child.stdout.off('data', onData);
      fn();
    };
    const timer = setTimeout(
      () =>
        finish(() =>
          reject(new Error(`${what}: timed out after ${timeoutMs}ms; saw: ${buf}`)),
        ),
      timeoutMs,
    );
    const onData = (chunk: Buffer) => {
      buf += chunk.toString();
      let nl: number;
      while ((nl = buf.indexOf('\n')) !== -1) {
        const line = buf.slice(0, nl);
        buf = buf.slice(nl + 1);
        if (match(line)) {
          finish(() => resolve(line));
          return;
        }
        onOtherLine?.(line);
      }
    };
    child.stdout.on('data', onData);
    child.once('exit', (code) =>
      finish(() => reject(new Error(`${what}: process exited early (${code})`))),
    );
    // Without this, spawn ENOENT is an uncaught exception, not a fixture failure.
    child.once('error', (err) =>
      finish(() => reject(new Error(`${what}: spawn failed: ${err.message}`))),
    );
  });
}

async function killTree(child: ChildProcessWithoutNullStreams): Promise<void> {
  // No pid = spawn itself failed; there is nothing to reap and no 'exit' coming.
  if (child.pid === undefined) return;
  if (child.exitCode !== null || child.signalCode !== null) return;
  const exited = new Promise<void>((r) => {
    child.once('exit', () => r());
    child.once('close', () => r());
  });
  child.kill('SIGTERM');
  const killer = setTimeout(() => child.kill('SIGKILL'), 3000);
  await exited;
  clearTimeout(killer);
}

/** One WS connection per worker; commands are id-multiplexed. */
class BridgeClient {
  private ws?: WebSocket;
  private nextId = 1;
  private pending = new Map<
    number,
    { resolve: (v: unknown) => void; reject: (e: unknown) => void }
  >();

  constructor(private readonly port: number) {}

  private async connect(): Promise<WebSocket> {
    if (this.ws && this.ws.readyState === WebSocket.OPEN) return this.ws;
    const ws = new WebSocket(`ws://127.0.0.1:${this.port}/bridge`);
    this.ws = ws;
    await new Promise<void>((resolve, reject) => {
      ws.once('open', () => resolve());
      ws.once('error', reject);
    });
    ws.on('message', (data) => {
      let frame: any;
      try {
        frame = JSON.parse(String(data));
      } catch {
        return;
      }
      if (frame?.id == null) return;
      const p = this.pending.get(frame.id);
      if (!p) return;
      this.pending.delete(frame.id);
      if ('err' in frame) p.reject(new Error(JSON.stringify(frame.err)));
      else p.resolve(frame.ok);
    });
    return ws;
  }

  async invoke<T>(cmd: string, args: Record<string, unknown> = {}): Promise<T> {
    const ws = await this.connect();
    const id = this.nextId++;
    return new Promise<T>((resolve, reject) => {
      this.pending.set(id, {
        resolve: resolve as (v: unknown) => void,
        reject,
      });
      ws.send(JSON.stringify({ id, cmd, args }));
    });
  }

  close(): void {
    this.ws?.close();
  }
}

/** Per-worker-process flag: a failure keeps the config dir for post-mortem. */
let workerHadFailure = false;

type WorkerFixtures = { harness: RealHarness };
type TestFixtures = { autoReset: void };

export const test = base.extend<TestFixtures, WorkerFixtures>({
  harness: [
    async ({}, use, workerInfo) => {
      // Everything below lives in one try/finally so teardown owns both children
      // (and the temp dir) no matter where setup fails.
      let sims: ChildProcessWithoutNullStreams | undefined;
      let app: ChildProcessWithoutNullStreams | undefined;
      let bridge: BridgeClient | undefined;
      let configDir: string | undefined;
      let simsStderr = '';
      let stderrBuf = '';

      try {
        sims = spawn(path.join(repoRoot, 'target/debug/flts-e2e-sims'), {
          // stdin stays piped and open: the sims exit on EOF.
          stdio: ['pipe', 'pipe', 'pipe'],
          env: process.env,
        }) as ChildProcessWithoutNullStreams;
        sims.on('error', () => {}); // surfaced via awaitStdoutLine
        sims.stdin.on('error', () => {}); // EPIPE on kill is expected
        sims.stderr.on(
          'data',
          (c) => (simsStderr = (simsStderr + c).slice(-8000)),
        );

        let ports: SimPorts;
        try {
          const line = await awaitStdoutLine(
            sims,
            'flts-e2e-sims port line',
            (l) => l.trim().startsWith('{'),
            10_000,
          );
          ports = JSON.parse(line);
        } catch (err) {
          throw new Error(
            `${(err as Error).message}\nsims stderr:\n${simsStderr}`,
          );
        }

        configDir = fs.mkdtempSync(path.join(os.tmpdir(), 'flts-e2e-'));
        fs.writeFileSync(
          path.join(configDir, 'config.json'),
          JSON.stringify(
            {
              targetLanguageId: 'eng',
              translationProvider: 'google',
              geminiApiKey: 'sim-key',
              openaiApiKey: 'sim-key',
              deepseekApiKey: 'sim-key',
              zaiApiKey: 'sim-key',
              // TranslationModel serializes as usize; 1 = Gemini25Flash.
              model: 1,
              ankiEndpoint: `http://127.0.0.1:${ports.anki}`,
              syncEnabled: false,
            },
            null,
            2,
          ),
        );

        app = spawn(path.join(repoRoot, 'target/debug/app'), {
          stdio: ['pipe', 'pipe', 'pipe'],
          env: {
            ...process.env,
            FLTS_E2E_BRIDGE_PORT: '0',
            FLTS_CONFIG_DIR: configDir,
            FLTS_GEMINI_BASE_URL: `http://127.0.0.1:${ports.llm}/v1beta/`,
            OPENAI_BASE_URL: `http://127.0.0.1:${ports.llm}/v1`,
            FLTS_DEEPSEEK_BASE_URL: `http://127.0.0.1:${ports.llm}`,
            FLTS_ZAI_BASE_URL: `http://127.0.0.1:${ports.llm}`,
            FLTS_LRCLIB_BASE_URL: `http://127.0.0.1:${ports.lrclib}`,
            FLTS_DISABLE_SYNC: '1',
            // Never the developer's real "FLTS-Spotify" keychain entry.
            FLTS_KEYRING_SERVICE: `FLTS-E2E-${path.basename(configDir)}`,
            FLTS_ANKI_SYNC_INTERVAL_SECS: '3600',
          },
        }) as ChildProcessWithoutNullStreams;
        app.on('error', () => {});
        app.stdin.on('error', () => {});
        app.stderr.on('data', (c) => (stderrBuf = (stderrBuf + c).slice(-64_000)));
        app.stdout.on('data', () => {});

        let bridgePort: number;
        try {
          const line = await awaitStdoutLine(
            app,
            'app bridge line',
            (l) => l.startsWith('FLTS_E2E_BRIDGE_LISTENING'),
            30_000,
          );
          bridgePort = JSON.parse(
            line.slice('FLTS_E2E_BRIDGE_LISTENING'.length).trim(),
          ).port;
        } catch (err) {
          throw new Error(`${(err as Error).message}\napp stderr:\n${stderrBuf}`);
        }

        bridge = new BridgeClient(bridgePort);
        const harness: RealHarness = {
          llm: new SimClient(`http://127.0.0.1:${ports.llm}`),
          lrclib: new SimClient(`http://127.0.0.1:${ports.lrclib}`),
          anki: new SimClient(`http://127.0.0.1:${ports.anki}`),
          bridgePort,
          configDir,
          appStderr: () => stderrBuf,
          invoke: (cmd, args) => bridge!.invoke(cmd, args),
        };
        try {
          // The app answers on the bridge before any test runs.
          await harness.invoke('get_config');
        } catch (err) {
          throw new Error(
            `bridge health check (get_config) failed: ${(err as Error).message}\napp stderr:\n${stderrBuf}`,
          );
        }

        setHarness(harness);
        await use(harness);
      } catch (err) {
        workerHadFailure = true;
        throw err;
      } finally {
        setHarness(undefined);
        bridge?.close();
        if (app) await killTree(app);
        if (sims) await killTree(sims);
        if (configDir) {
          if (workerHadFailure) {
            console.log(
              `[worker ${workerInfo.workerIndex}] config dir kept: ${configDir}`,
            );
          } else {
            fs.rmSync(configDir, { recursive: true, force: true });
          }
        }
      }
    },
    { scope: 'worker', auto: true, timeout: 120_000 },
  ],

  autoReset: [
    async ({ harness }, use, testInfo) => {
      await Promise.all([
        harness.llm.reset(),
        harness.lrclib.reset(),
        harness.anki.reset(),
      ]);
      const books =
        await harness.invoke<Array<{ id: string }>>('list_books');
      for (const b of books) {
        await harness.invoke('delete_book', { bookId: b.id });
      }

      await use();

      if (testInfo.status !== testInfo.expectedStatus) {
        workerHadFailure = true;
        await testInfo.attach('app-stderr', {
          body: harness.appStderr(),
          contentType: 'text/plain',
        });
        for (const [name, sim] of [
          ['llm', harness.llm],
          ['lrclib', harness.lrclib],
          ['anki', harness.anki],
        ] as const) {
          await testInfo
            .attach(`sim-${name}`, {
              body: JSON.stringify(
                { rules: sim.rules(), requests: await sim.requests() },
                null,
                2,
              ),
              contentType: 'application/json',
            })
            .catch(() => {});
        }
        await testInfo.attach('config-dir', {
          body: harness.configDir,
          contentType: 'text/plain',
        });
      }
    },
    { auto: true },
  ],

  page: async ({ page, harness }, use) => {
    await page.addInitScript(
      (port) => ((window as any).__FLTS_BRIDGE_PORT = port),
      harness.bridgePort,
    );
    await use(page);
  },
});
