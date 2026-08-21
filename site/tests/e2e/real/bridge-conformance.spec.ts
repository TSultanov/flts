// Every bridged command invoked once against the real backend. A command
// passes when it SETTLES: resolves, or rejects with a serialized error value.
// Transport failures, timeouts and "unknown command" are conformance failures.
import fs from 'node:fs';
import net from 'node:net';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { test, expect } from '../../real/fixtures';

/** Must clear the backend's own 30s keychain bound. */
const KEYCHAIN_BOUND_MS = 45_000;

/** Mirror of `COMMANDS` in site/src-tauri/src/bridge.rs. Keep in lockstep. */
const ALL_COMMANDS = [
  'get_models',
  'get_languages',
  'parse_language_id',
  'get_config',
  'get_library_root',
  'reveal_library_root',
  'update_config',
  'purge_gemini_caches',
  'get_anki_sync_status',
  'sync_anki_now',
  'get_sync_status',
  'sync_get_this_device',
  'sync_web_ui_url',
  'sync_set_device_name',
  'sync_wake',
  'sync_set_enabled',
  'sync_list_devices',
  'sync_list_pending',
  'sync_add_device',
  'sync_remove_device',
  'translate_paragraph',
  'translate_chapter',
  'get_paragraph_translation_activity',
  'list_paragraph_translation_activity',
  'list_books',
  'list_book_chapters',
  'get_book_chapter_paragraph_ids',
  'get_paragraph_view',
  'get_paragraph_originals_batch',
  'get_paragraph_translations_batch',
  'get_translation_providers',
  'get_word_info',
  'import_plain_text',
  'parse_epub',
  'import_epub',
  'get_book_reading_state',
  'get_book_summary_status',
  'save_book_reading_state',
  'move_book',
  'delete_book',
  'get_system_definition',
  'show_system_dictionary',
  'start_spotify_watcher',
  'stop_spotify_watcher',
  'get_now_playing',
  'get_track_lyrics_state',
  'spotify_web_connect',
  'spotify_web_disconnect',
  'spotify_web_status',
  'spotify_web_get_queue',
  'open_external_url',
] as const;

type Status = 'resolved' | 'rejected' | 'timeout' | 'transport';
type Outcome = { cmd: string; status: Status; value: string; raw?: unknown };

const covered = new Set<string>();

/** Invokes through the in-page shim so the real IPC path is exercised. */
async function call(
  page: import('@playwright/test').Page,
  cmd: string,
  args: Record<string, unknown> = {},
  timeoutMs = 30_000,
): Promise<Outcome> {
  const res = await page.evaluate(
    async ({ cmd, args, timeoutMs }) => {
      const invoke = (window as any).__bridgeDebugInvoke(cmd, args);
      let timer: ReturnType<typeof setTimeout> | undefined;
      const timeout = new Promise<'__timeout'>((r) => {
        timer = setTimeout(() => r('__timeout'), timeoutMs);
      });
      try {
        const raw = await Promise.race([invoke, timeout]);
        if (raw === '__timeout') return { status: 'timeout', value: '' };
        let value = '';
        try {
          value = JSON.stringify(raw) ?? String(raw);
        } catch {
          value = '<unserializable>';
        }
        return { status: 'resolved', value: value.slice(0, 300), raw };
      } catch (err) {
        // The shim rejects with the serialized `err` payload; an Error means
        // the transport itself failed (socket closed / send threw).
        if (err instanceof Error)
          return { status: 'transport', value: err.message };
        return { status: 'rejected', value: String(JSON.stringify(err)).slice(0, 300) };
      } finally {
        clearTimeout(timer);
      }
    },
    { cmd, args, timeoutMs },
  );
  covered.add(cmd);
  const out = { cmd, ...res } as Outcome;
  console.log(`[bridge] ${out.cmd} -> ${out.status} ${out.value}`);
  return out;
}

function expectSettled(outcomes: Outcome[]): void {
  const bad = outcomes
    .filter(
      (o) =>
        o.status === 'timeout' ||
        o.status === 'transport' ||
        // Table drift, and arg-casing drift: every command here gets valid args.
        (o.status === 'rejected' &&
          /unknown command|^"?bad args:/i.test(o.value)),
    )
    .map((o) => `${o.cmd}: ${o.status} ${o.value}`);
  expect(bad).toEqual([]);
}

async function openPage(page: import('@playwright/test').Page): Promise<void> {
  await page.goto('/');
  await page.waitForFunction(() => !!(window as any).__bridgeDebugInvoke);
}

const SAMPLE_TEXT =
  'Der Hund lief schnell. Die Katze schlief.\n\nEin zweiter Absatz folgt hier.';

test.describe('bridge conformance', () => {
  test.describe.configure({ mode: 'serial' });

  test('read-only commands settle', async ({ page }) => {
    // Two keychain-bound commands can each burn 45s before anything else runs.
    test.setTimeout(300_000);
    await openPage(page);

    // Sentinel: proves the err-shape a drifted table would produce.
    const sentinel = await call(page, '__no_such_command__');
    expect(sentinel.status).toBe('rejected');
    expect(sentinel.value).toMatch(/unknown command/i);
    covered.delete('__no_such_command__');

    const table: Array<[string, Record<string, unknown>?, number?]> = [
      ['get_models'],
      ['get_languages'],
      ['get_translation_providers'],
      ['parse_language_id', { code: 'eng' }],
      ['get_config'],
      ['get_library_root'],
      ['list_books'],
      ['list_paragraph_translation_activity'],
      ['get_anki_sync_status'],
      ['sync_anki_now'],
      ['purge_gemini_caches'],
      // Sync is off (FLTS_DISABLE_SYNC=1): these return empties or errors.
      ['get_sync_status'],
      ['sync_get_this_device'],
      ['sync_web_ui_url'],
      ['sync_list_devices'],
      ['sync_list_pending'],
      ['sync_wake'],
      // No credentials: rejections are the expected shape.
      ['get_now_playing'],
      ['get_track_lyrics_state', { trackId: 'sim:track', targetLang: 'eng', model: 'models/gemini-2.5-flash' }],
      ['spotify_web_status'],
      ['spotify_web_get_queue'],
      // Touches the keychain, whose own bound is 30s.
      ['spotify_web_disconnect', {}, KEYCHAIN_BOUND_MS],
      ['start_spotify_watcher'],
      ['stop_spotify_watcher'],
      ['get_system_definition', { word: 'Hund', sourceLang: 'deu', targetLang: 'eng' }],
      ['show_system_dictionary', { word: 'Hund' }],
      // Non-http on purpose: asserts the guard instead of launching a browser.
      ['open_external_url', { url: 'ftp://example.invalid/' }],
      ['reveal_library_root'],
    ];

    const outcomes: Outcome[] = [];
    for (const [cmd, args, bound] of table)
      outcomes.push(await call(page, cmd, args, bound));

    // Holding the OAuth loopback port makes connect fail its bind and return
    // instead of parking for 300s on a browser callback.
    const squatter = net.createServer();
    await new Promise<void>((r) => squatter.listen(53682, '127.0.0.1', r));
    try {
      outcomes.push(
        await call(page, 'spotify_web_connect', { clientId: 'e2e-client-id' }),
      );
    } finally {
      await new Promise<void>((r) => squatter.close(() => r()));
    }

    expectSettled(outcomes);
  });

  test('entity-dependent commands settle', async ({ page }) => {
    test.setTimeout(180_000);
    await openPage(page);
    const outcomes: Outcome[] = [];

    const imported = await call(page, 'import_plain_text', {
      title: 'Conformance Book',
      text: SAMPLE_TEXT,
      sourceLanguageId: 'deu',
    });
    outcomes.push(imported);
    expect(imported.status).toBe('resolved');
    const bookId = imported.raw as string;

    const chapters = await call(page, 'list_book_chapters', { bookId });
    outcomes.push(chapters);
    const chapterId = (chapters.raw as Array<{ id: number }>)?.[0]?.id ?? 0;

    const pids = await call(page, 'get_book_chapter_paragraph_ids', {
      bookId,
      chapterId,
    });
    outcomes.push(pids);
    const paragraphIds = (pids.raw as number[]) ?? [];
    const paragraphId = paragraphIds[0] ?? 0;

    const table: Array<[string, Record<string, unknown>?, number?]> = [
      ['get_paragraph_view', { bookId, paragraphId }],
      ['get_paragraph_originals_batch', { bookId, paragraphIds }],
      ['get_paragraph_translations_batch', { bookId, paragraphIds }],
      ['get_word_info', { bookId, paragraphId, sentenceId: 0, wordId: 0 }],
      ['get_book_reading_state', { bookId }],
      ['get_book_summary_status', { bookId }],
      ['get_paragraph_translation_activity', { bookId, paragraphId }],
      ['save_book_reading_state', { bookId, chapterId, paragraphId, pageOffset: 0 }],
      // Hits the LLM sim; its fallback answers unscripted requests.
      ['translate_paragraph', { bookId, paragraphId, model: 'models/gemini-2.5-flash', useCache: false }],
      ['translate_chapter', { bookId, chapterId, model: 'models/gemini-2.5-flash', useCache: false }],
      [
        'import_epub',
        {
          book: {
            title: 'Conformance Epub',
            chapters: [
              {
                title: 'Kapitel 1',
                paragraphs: [{ text: 'Ein Satz.', html: '<p>Ein Satz.</p>' }],
              },
            ],
          },
          sourceLanguageId: 'deu',
        },
      ],
    ];

    for (const [cmd, args, bound] of table)
      outcomes.push(await call(page, cmd, args, bound));
    expectSettled(outcomes);
  });

  test('mutating commands settle', async ({ page }) => {
    test.setTimeout(180_000);
    await openPage(page);
    const outcomes: Outcome[] = [];

    const throwaway = await call(page, 'import_plain_text', {
      title: 'Throwaway',
      text: 'Ein Satz.',
      sourceLanguageId: 'deu',
    });
    expect(throwaway.status).toBe('resolved');
    const bookId = throwaway.raw as string;

    const cfg = await call(page, 'get_config');
    expect(cfg.status).toBe('resolved');

    const table: Array<[string, Record<string, unknown>?, number?]> = [
      ['sync_set_device_name', { name: 'e2e-conformance' }],
      ['sync_set_enabled', { enabled: false }],
      ['sync_add_device', { deviceId: 'AAAAAAA-BBBBBBB-CCCCCCC-DDDDDDD', name: 'peer' }],
      ['sync_remove_device', { deviceId: 'AAAAAAA-BBBBBBB-CCCCCCC-DDDDDDD' }],
      ['parse_epub', { epubBase64: 'bm90IGFuIGVwdWI=' }], // "not an epub"
      ['move_book', { bookId, path: ['conformance'] }],
      ['delete_book', { bookId }],
      // Identity round-trip, last: it re-evaluates the whole app config.
      ['update_config', { config: cfg.raw }],
    ];

    for (const [cmd, args, bound] of table)
      outcomes.push(await call(page, cmd, args, bound));
    expectSettled(outcomes);
  });

  test('every bridged command was covered', async () => {
    expect(new Set(ALL_COMMANDS).size).toBe(ALL_COMMANDS.length);
    expect([...ALL_COMMANDS].filter((c) => !covered.has(c))).toEqual([]);
    expect([...covered].filter((c) => !ALL_COMMANDS.includes(c as never))).toEqual([]);

    // A command added to the bridge must fail here, not go untested.
    const bridgeRs = path.resolve(
      path.dirname(fileURLToPath(import.meta.url)),
      '../../../src-tauri/src/bridge.rs',
    );
    const src = fs.readFileSync(bridgeRs, 'utf8');
    const block = /pub const COMMANDS: &\[&str\] = &\[([\s\S]*?)\];/.exec(src);
    expect(block, `COMMANDS block not found in ${bridgeRs}`).not.toBeNull();
    const declared = [...block![1].matchAll(/"([a-z0-9_]+)"/g)].map((m) => m[1]);
    expect([...declared].sort()).toEqual([...ALL_COMMANDS].sort());
  });
});
