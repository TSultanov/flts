import fs from "node:fs";
import path from "node:path";
import type { CDPSession, Page } from "@playwright/test";
import { test, expect } from "../../real/fixtures";
import type { RealHarness } from "../../real/fixtures";
import { MODEL, drained } from "../../real/spec-helpers";

/**
 * Where the time goes between asking for a long chapter and seeing it.
 * A measurement, not a regression gate: run with FLTS_PERF=1.
 */

const LONG_PARAGRAPHS = Number(process.env.FLTS_PERF_PARAGRAPHS ?? 600);
const ITERATIONS = Number(process.env.FLTS_PERF_ITERATIONS ?? 5);
const LONG_CHAPTER = 1;

// Deterministic, so runs compare.
function rng(seed: number): () => number {
  let s = seed >>> 0;
  return () => {
    s = (Math.imul(s, 1664525) + 1013904223) >>> 0;
    return s / 0x100000000;
  };
}

const SYLLABLES = [
  "ge",
  "ver",
  "be",
  "ent",
  "schaft",
  "lich",
  "keit",
  "ung",
  "über",
  "für",
  "wäh",
  "rend",
  "zwi",
  "schen",
  "mög",
  "tra",
  "gen",
  "hal",
  "ten",
  "spre",
  "chen",
  "stra",
  "ße",
  "kön",
  "nen",
  "wis",
  "sen",
  "blei",
  "ben",
  "nach",
];

function vocabulary(random: () => number): string[] {
  const words = new Set<string>();
  while (words.size < 500) {
    const n = 1 + Math.floor(random() * 3);
    let w = "";
    for (let i = 0; i < n; i++) {
      w += SYLLABLES[Math.floor(random() * SYLLABLES.length)];
    }
    words.add(w);
  }
  return [...words];
}

/** A letters-only token unique to paragraph `i`: the script's match key. */
function marker(nonce: string, i: number): string {
  let s = "";
  let n = i;
  do {
    s = String.fromCharCode(97 + (n % 26)) + s;
    n = Math.floor(n / 26);
  } while (n > 0);
  return `zq${nonce}${s}zq`;
}

type Paragraph = {
  text: string;
  html: string;
  marker: string;
  translation: unknown;
};

function wordEntry(o: string, k: number) {
  return {
    o,
    t: k % 4 === 0 ? [`tr-${o}`, `alt-${o}`] : [`tr-${o}`],
    n: k % 9 === 0 ? `note on ${o}` : null,
    p: false,
    g: {
      lf: o.toLowerCase(),
      lt: `tr-${o}`,
      pos: k % 3 === 0 ? "verb" : "common_noun",
      pl: k % 2 === 0 ? "singular" : null,
      pe: null,
      te: k % 3 === 0 ? "present" : null,
      ca: k % 5 === 0 ? "dative" : null,
      ot: null,
    },
  };
}

const punctuation = (o: string) => ({
  o,
  t: [],
  n: null,
  p: true,
  g: {
    lf: o,
    lt: o,
    pos: "punctuation",
    pl: null,
    pe: null,
    te: null,
    ca: null,
    ot: null,
  },
});

/** Dialogue lines and prose blocks, with commas and emphasis runs. */
function paragraph(
  random: () => number,
  vocab: string[],
  mark: string,
): Paragraph {
  const dialogue = random() < 0.35;
  const sentences = dialogue ? 1 : 2 + Math.floor(random() * 5);
  const text: string[] = [];
  const html: string[] = [];
  const s: unknown[] = [];
  let k = 0;
  for (let si = 0; si < sentences; si++) {
    const count = dialogue
      ? 4 + Math.floor(random() * 8)
      : 10 + Math.floor(random() * 18);
    const wl: unknown[] = [];
    const emphasisAt = random() < 0.15 ? Math.floor(random() * count) : -1;
    for (let wi = 0; wi < count; wi++) {
      let w =
        si === 0 && wi === 0
          ? mark
          : vocab[Math.floor(random() * vocab.length)];
      if (wi === 0) w = w[0].toUpperCase() + w.slice(1);
      const comma = wi > 2 && wi < count - 1 && random() < 0.1;
      const last = wi === count - 1;
      const tail = last ? "." : comma ? "," : "";
      text.push(w + tail);
      html.push((wi === emphasisAt ? `<em>${w}</em>` : w) + tail);
      wl.push(wordEntry(w, k++));
      if (tail) wl.push(punctuation(tail));
    }
    s.push({ ft: `Sentence ${si} of ${mark}.`, wl });
  }
  return {
    text: text.join(" "),
    html: html.join(" "),
    marker: mark[0].toUpperCase() + mark.slice(1),
    translation: { s },
  };
}

type Seeded = { bookId: string; longIds: number[] };

async function seedLongBook(h: RealHarness): Promise<Seeded> {
  const random = rng(20261112);
  const vocab = vocabulary(random);
  // The translation cache outlives the config dir, so markers must be fresh.
  const nonce = Date.now()
    .toString(26)
    .replace(/\d/g, (d) => "abcdefghij"[Number(d)]);
  let next = 0;
  const chapter = (n: number) =>
    Array.from({ length: n }, () =>
      paragraph(random, vocab, marker(nonce, next++)),
    );
  const chapters = [chapter(30), chapter(LONG_PARAGRAPHS), chapter(30)];

  await h.llm.seed({
    scripts: chapters[LONG_CHAPTER].map((p) => ({
      matchSubstring: p.marker,
      translation: p.translation,
      chunks: 1,
    })),
    fallback: "minimal",
  });
  const bookId = await h.invoke<string>("import_epub", {
    book: {
      title: "perf-long-chapter",
      chapters: chapters.map((paragraphs, i) => ({
        title: `Chapter ${i}`,
        paragraphs: paragraphs.map(({ text, html }) => ({ text, html })),
      })),
    },
    sourceLanguageId: "deu",
  });
  await h.invoke("translate_chapter", {
    bookId,
    chapterId: LONG_CHAPTER,
    model: MODEL,
    useCache: false,
  });
  const longIds = await h.invoke<number[]>("get_book_chapter_paragraph_ids", {
    bookId,
    chapterId: LONG_CHAPTER,
  });
  await expect
    .poll(
      async () => {
        const chapters = await h.invoke<
          Array<{ id: number; translationRatio: number }>
        >("list_book_chapters", { bookId });
        const busy = (await drained(h)).length;
        const ratio = chapters.find(
          (c) => c.id === LONG_CHAPTER,
        )!.translationRatio;
        return busy === 0 && ratio >= 0.999;
      },
      { timeout: 600_000, intervals: [1000] },
    )
    .toBe(true);
  return { bookId, longIds };
}

/** Runs in the page before the app: frames, bridge traffic and long tasks. */
function instrument(): void {
  const perf = {
    t0: 0,
    armed: false,
    revealed: null as number | null,
    // Geometry reads force layout; off when the layouts themselves are studied.
    probe: true,
    oldContainer: null as Element | null,
    frames: [] as Array<Record<string, number | boolean>>,
    invokes: new Map<number, Record<string, number | string>>(),
    events: [] as Array<{ t: number; name: string }>,
    longTasks: [] as Array<{ t: number; ms: number }>,
  };
  (window as any).__perf = perf;

  const Native = window.WebSocket;
  const nativeSend = Native.prototype.send;
  Native.prototype.send = function (data: any) {
    if (perf.armed && typeof data === "string") {
      try {
        const f = JSON.parse(data);
        if (f?.id != null) {
          perf.invokes.set(f.id, {
            cmd: f.cmd,
            n: f.args?.paragraphIds?.length ?? 0,
            sent: performance.now() - perf.t0,
          });
        }
      } catch {
        /* not ours */
      }
    }
    return nativeSend.call(this, data);
  };
  (window as any).WebSocket = new Proxy(Native, {
    construct(target, args) {
      const ws = new target(...(args as [string]));
      // Registered before the app's onmessage, so `recv` precedes handling.
      ws.addEventListener("message", (m: MessageEvent) => {
        if (!perf.armed) return;
        const at = performance.now() - perf.t0;
        const raw = String(m.data);
        const head = raw.slice(0, 80);
        const id = /^\{"id":(\d+)/.exec(head);
        if (id) {
          const rec = perf.invokes.get(Number(id[1]));
          if (rec) {
            rec.recv = at;
            rec.bytes = raw.length;
            // The app's handler runs next in this same task.
            queueMicrotask(() => {
              rec.parsed = performance.now() - perf.t0;
            });
            setTimeout(() => {
              rec.applied = performance.now() - perf.t0;
            });
          }
        } else {
          const name = /"event":"([^"]+)"/.exec(head);
          if (name) perf.events.push({ t: at, name: name[1] });
        }
      });
      return ws;
    },
  });

  try {
    new PerformanceObserver((list) => {
      if (!perf.armed) return;
      for (const e of list.getEntries()) {
        perf.longTasks.push({ t: e.startTime - perf.t0, ms: e.duration });
      }
    }).observe({ entryTypes: ["longtask"] });
  } catch {
    /* unsupported */
  }

  // The frame probe sees the reveal a frame late; this is when it was asked for.
  new MutationObserver((records) => {
    if (!perf.armed || perf.revealed !== null) return;
    for (const r of records) {
      const el = r.target as Element;
      if (
        el !== perf.oldContainer &&
        el.classList.contains("paragraphs-container") &&
        el.classList.contains("is-ready")
      ) {
        perf.revealed = performance.now() - perf.t0;
      }
    }
  }).observe(document, {
    subtree: true,
    attributes: true,
    attributeFilter: ["class"],
  });

  const frame = () => {
    if (perf.armed) {
      const c = document.querySelector(".paragraphs-container");
      const fresh = c != null && c !== perf.oldContainer;
      const rec: Record<string, number | boolean> = {
        t: performance.now() - perf.t0,
        container: fresh,
        ready: false,
        paragraphs: 0,
        text: false,
        words: false,
      };
      if (fresh) {
        rec.ready = c.classList.contains("is-ready");
        rec.paragraphs = c.getElementsByClassName("paragraph-wrapper").length;
        const r = perf.probe ? c.getBoundingClientRect() : null;
        for (const fy of r ? [0.2, 0.5, 0.8] : []) {
          const hit = document.elementFromPoint(
            r!.left + r.width / 2,
            r!.top + r!.height * fy,
          );
          const wrapper = hit?.closest(".paragraph-wrapper");
          if (!wrapper || !c.contains(wrapper)) continue;
          if ((wrapper.textContent ?? "").trim().length > 0) rec.text = true;
          if (wrapper.querySelector(".word-span")) rec.words = true;
        }
      }
      perf.frames.push(rec);
    }
    requestAnimationFrame(frame);
  };
  requestAnimationFrame(frame);

  // Capture phase: the stamp precedes the router's own click handling.
  window.addEventListener(
    "click",
    (e) => {
      const a = (e.target as Element | null)?.closest?.("a[data-perf-arm]");
      if (!a) return;
      perf.oldContainer = document.querySelector(".paragraphs-container");
      perf.frames = [];
      perf.invokes = new Map();
      perf.events = [];
      perf.longTasks = [];
      perf.revealed = null;
      perf.t0 = performance.now();
      perf.armed = true;
    },
    true,
  );
}

type Run = {
  milestones: Record<string, number | null>;
  frames: Array<Record<string, number | boolean>>;
  invokes: Array<Record<string, number | string>>;
  events: Array<{ t: number; name: string }>;
  longTasks: Array<{ t: number; ms: number }>;
  cpu: Record<string, number>;
};

const CPU_KEYS = [
  "TaskDuration",
  "ScriptDuration",
  "LayoutDuration",
  "RecalcStyleDuration",
  "LayoutCount",
  "RecalcStyleCount",
];

async function cpuMetrics(
  cdp: CDPSession | null,
): Promise<Record<string, number>> {
  // Chromium only.
  if (!cdp) return Object.fromEntries(CPU_KEYS.map((k) => [k, NaN]));
  const { metrics } = await cdp.send("Performance.getMetrics");
  return Object.fromEntries(
    metrics
      .filter((m) => CPU_KEYS.includes(m.name))
      .map((m) => [m.name, m.value]),
  );
}

/** Clicks `link` and records until translated words are on screen. */
async function measureClick(
  page: Page,
  cdp: CDPSession | null,
  link: ReturnType<Page["locator"]>,
): Promise<Run> {
  await link.evaluate((a) => a.setAttribute("data-perf-arm", ""));
  const before = await cpuMetrics(cdp);
  await link.click();
  await page.waitForFunction(
    () => {
      const p = (window as any).__perf;
      const f = p.frames;
      const last = f[f.length - 1];
      return last && last.ready && (last.words || !p.probe);
    },
    undefined,
    { timeout: 30_000 },
  );
  // The tail: batches and mounts still landing after first sight.
  await page.waitForTimeout(1500);
  const after = await cpuMetrics(cdp);
  const raw = await page.evaluate(() => {
    const p = (window as any).__perf;
    p.armed = false;
    return {
      frames: p.frames,
      revealed: p.revealed as number | null,
      invokes: [...p.invokes.values()],
      events: p.events,
      longTasks: p.longTasks,
    };
  });
  return {
    ...raw,
    milestones: milestonesOf(raw),
    cpu: Object.fromEntries(
      CPU_KEYS.map((k) => [
        k,
        k.endsWith("Count")
          ? after[k] - before[k]
          : (after[k] - before[k]) * 1000,
      ]),
    ),
  };
}

function milestonesOf(raw: {
  frames: Array<Record<string, number | boolean>>;
  revealed: number | null;
}): Record<string, number | null> {
  const first = (pred: (f: Record<string, number | boolean>) => boolean) => {
    const f = raw.frames.find(pred);
    return f ? (f.t as number) : null;
  };
  return {
    firstFrame: first(() => true),
    oldGone: first((f) => f.container === true || f.paragraphs === 0),
    container: first((f) => f.container === true),
    paragraphIds: first((f) => (f.paragraphs as number) > 0),
    textLaidOut: first((f) => f.text === true),
    revealAsked: raw.revealed,
    visible: first((f) => f.ready === true),
    visibleWithText: first((f) => f.ready === true && f.text === true),
    words: first((f) => f.ready === true && f.words === true),
  };
}

/** Longest frame before the reveal was asked for, and after. */
function longestFrames(run: Run): [hidden: number, shown: number] {
  const at = run.frames.map((f) => f.t as number);
  const reveal = run.milestones.revealAsked ?? 0;
  let hidden = 0;
  let shown = 0;
  for (let i = 1; i < at.length; i++) {
    const gap = at[i] - at[i - 1];
    if (at[i - 1] < reveal) hidden = Math.max(hidden, gap);
    else shown = Math.max(shown, gap);
  }
  return [Math.round(hidden), Math.round(shown)];
}

const median = (xs: number[]) => {
  const s = [...xs].sort((a, b) => a - b);
  return s.length ? s[Math.floor(s.length / 2)] : NaN;
};

function summarize(name: string, runs: Run[]): string {
  const lines: string[] = [
    `\n=== ${name} (${runs.length} runs, median ms) ===`,
  ];
  for (const key of Object.keys(runs[0].milestones)) {
    const xs = runs
      .map((r) => r.milestones[key])
      .filter((x): x is number => x != null);
    lines.push(
      `  ${key.padEnd(16)} ${median(xs).toFixed(0).padStart(6)}   [${xs
        .map((x) => x.toFixed(0))
        .join(", ")}]`,
    );
  }
  lines.push("  -- main thread until 1.5 s after words --");
  for (const key of CPU_KEYS) {
    lines.push(
      `  ${key.padEnd(20)} ${median(runs.map((r) => r.cpu[key])).toFixed(0)}`,
    );
  }
  const run = runs[Math.floor(runs.length / 2)];
  lines.push("  -- one run: frames longer than 25 ms --");
  let prev = 0;
  for (const f of run.frames) {
    const t = f.t as number;
    if (t - prev > 25) {
      lines.push(
        `  ${prev.toFixed(0).padStart(6)} → ${t.toFixed(0).padStart(6)}  (${(t - prev).toFixed(0)} ms) paragraphs=${f.paragraphs} ready=${f.ready} text=${f.text} words=${f.words}`,
      );
    }
    prev = t;
  }
  lines.push(
    "  -- one run: bridge calls (sent → received → applied, bytes) --",
  );
  const byCmd = new Map<string, Array<Record<string, number | string>>>();
  for (const i of run.invokes) {
    const list = byCmd.get(i.cmd as string) ?? [];
    list.push(i);
    byCmd.set(i.cmd as string, list);
  }
  for (const [cmd, list] of byCmd) {
    const sent = list.map((i) => i.sent as number);
    const applied = list.map((i) => (i.applied ?? i.recv ?? NaN) as number);
    const wait = list.map(
      (i) => ((i.recv as number) ?? NaN) - (i.sent as number),
    );
    const handle = list.map(
      (i) => ((i.parsed as number) ?? NaN) - ((i.recv as number) ?? NaN),
    );
    const bytes = list.reduce((a, i) => a + ((i.bytes as number) ?? 0), 0);
    lines.push(
      `  ${cmd.padEnd(36)} ×${String(list.length).padStart(3)} first sent ${Math.min(...sent).toFixed(0)}, last applied ${Math.max(...applied).toFixed(0)}, wait median ${median(wait).toFixed(0)} max ${Math.max(...wait).toFixed(0)}, handler median ${median(handle).toFixed(1)} max ${Math.max(...handle).toFixed(1)}, ${(bytes / 1024).toFixed(0)} KiB`,
    );
  }
  return lines.join("\n");
}

test.describe("chapter open timing", () => {
  test.skip(!process.env.FLTS_PERF, "measurement; run with FLTS_PERF=1");
  test.describe.configure({ mode: "serial" });

  test("a long translated chapter", async ({ page, harness }, testInfo) => {
    test.setTimeout(1_200_000);
    const seedStart = Date.now();
    const { bookId, longIds } = await seedLongBook(harness);
    console.log(
      `seeded ${longIds.length} translated paragraphs in ${((Date.now() - seedStart) / 1000).toFixed(0)} s`,
    );
    const shortIds = await harness.invoke<number[]>(
      "get_book_chapter_paragraph_ids",
      { bookId, chapterId: 0 },
    );

    // The backend alone, without a page competing for the bridge.
    const timed = async (label: string, cmd: string, ids: number[]) => {
      const xs: number[] = [];
      for (let i = 0; i < 5; i++) {
        const at = performance.now();
        await harness.invoke(cmd, { bookId, paragraphIds: ids });
        xs.push(performance.now() - at);
      }
      console.log(`backend ${label}: median ${median(xs).toFixed(1)} ms`);
    };
    await timed(
      "20 originals",
      "get_paragraph_originals_batch",
      longIds.slice(0, 20),
    );
    await timed("all originals", "get_paragraph_originals_batch", longIds);
    await timed(
      "20 translations",
      "get_paragraph_translations_batch",
      longIds.slice(0, 20),
    );
    await timed(
      "all translations",
      "get_paragraph_translations_batch",
      longIds,
    );

    await page.addInitScript(instrument);
    const cdp =
      testInfo.project.name === "real"
        ? await page.context().newCDPSession(page)
        : null;
    await cdp?.send("Performance.enable");

    const report: Record<string, Run[]> = {};

    /** Leaves the reader in the short chapter with the saved position given. */
    const settleAt = async (chapterId: number, paragraphId: number) => {
      // Off the app first: a departing chapter view saves its own position.
      await page.goto("about:blank");
      await page.waitForTimeout(600);
      await harness.invoke("save_book_reading_state", {
        bookId,
        chapterId,
        paragraphId,
        pageOffset: 0,
      });
    };

    const viaPanel = async (): Promise<Run> => {
      await page.goto(`/book/${bookId}/0`);
      await expect(
        page.locator(".paragraphs-container.is-ready"),
      ).toBeVisible();
      await page.waitForTimeout(800);
      await page.getByTestId("chapters-panel-handle").click();
      const link = page
        .getByTestId("chapter-row")
        .nth(LONG_CHAPTER)
        .locator("a");
      await expect(link).toBeVisible();
      await page.waitForTimeout(400);
      return measureClick(page, cdp, link);
    };

    const scenarios: Array<[string, () => Promise<Run>]> = [
      [
        "switch chapter, from its start",
        async () => {
          await settleAt(0, shortIds[0]);
          return viaPanel();
        },
      ],
      [
        "switch chapter, restoring a position 3/4 in",
        async () => {
          await settleAt(
            LONG_CHAPTER,
            longIds[Math.floor(longIds.length * 0.75)],
          );
          return viaPanel();
        },
      ],
      [
        "open from the library, restoring a position 3/4 in",
        async () => {
          await settleAt(
            LONG_CHAPTER,
            longIds[Math.floor(longIds.length * 0.75)],
          );
          await page.goto("/");
          const link = page.locator(`a[href="/book/${bookId}"]`);
          await expect(link).toBeAttached();
          await page.waitForTimeout(800);
          return measureClick(page, cdp, link);
        },
      ],
    ];

    // The view remembers where a chapter's paragraphs fell. "First time"
    // runs open it with that forgotten, "again" with it kept.
    await page.addInitScript(() => {
      try {
        if (localStorage.getItem("perf.forget")) {
          localStorage.removeItem("flts.chapterLayout.v2");
        }
      } catch {
        /* about:blank has no storage */
      }
    });
    const remember = async (kept: boolean) => {
      await page.goto("/");
      await page.evaluate((kept) => {
        if (kept) localStorage.removeItem("perf.forget");
        else localStorage.setItem("perf.forget", "1");
      }, kept);
    };

    for (const kept of [false, true]) {
      await remember(kept);
      for (const [name, run] of scenarios) {
        const title = `${name} — ${kept ? "again" : "first time"}`;
        const runs: Run[] = [];
        // One unrecorded pass warms caches on both sides.
        await run();
        for (let i = 0; i < ITERATIONS; i++) runs.push(await run());
        report[title] = runs;
        console.log(summarize(title, runs));
      }
    }

    // The reader is now 3/4 into the long chapter. What a flip and a tap
    // cost there: the longest gap between two frames, 17 ms being none.
    const interact = async (
      label: string,
      times: number,
      action: (i: number) => Promise<void>,
    ) => {
      const gaps: number[] = [];
      const layouts: number[] = [];
      for (let i = 0; i < times; i++) {
        await page.evaluate(() => {
          const gap = { last: performance.now(), max: 0, on: true };
          (window as any).__gap = gap;
          const tick = () => {
            if (!gap.on) return;
            const now = performance.now();
            gap.max = Math.max(gap.max, now - gap.last);
            gap.last = now;
            requestAnimationFrame(tick);
          };
          requestAnimationFrame(tick);
        });
        const before = await cpuMetrics(cdp);
        await action(i);
        await page.waitForTimeout(600);
        const after = await cpuMetrics(cdp);
        gaps.push(
          await page.evaluate(() => {
            const gap = (window as any).__gap;
            gap.on = false;
            return gap.max as number;
          }),
        );
        layouts.push((after.LayoutDuration - before.LayoutDuration) * 1000);
      }
      console.log(
        `  ${label.padEnd(22)} longest frame: median ${median(gaps).toFixed(0)} ms, worst ${Math.max(...gaps).toFixed(0)} ms; layout median ${median(layouts).toFixed(0)} ms`,
      );
      return { gaps, layouts };
    };
    const flip = (pages: number) => () =>
      page.locator(".paragraphs-container").evaluate((c, n) => {
        c.scrollBy({ left: n * c.clientWidth, behavior: "smooth" });
      }, pages);
    const tap = async (i: number) => {
      const at = await page.evaluate((nth) => {
        const c = document.querySelector(".paragraphs-container")!;
        const frame = c.getBoundingClientRect();
        const words = [...c.querySelectorAll(".word-span")].filter((w) => {
          const r = w.getBoundingClientRect();
          return (
            r.left > frame.left &&
            r.right < frame.right &&
            r.top > frame.top + 40 &&
            r.bottom < frame.bottom - 40
          );
        });
        const r = words[5 + nth * 7]?.getBoundingClientRect();
        return r ? { x: r.left + r.width / 2, y: r.top + r.height / 2 } : null;
      }, i);
      if (at) await page.mouse.click(at.x, at.y);
    };
    const profiled = cdp && process.env.FLTS_PERF_PROFILE === "interact";
    const traced = cdp && process.env.FLTS_PERF_TRACE === "interact";
    if (traced) {
      await page
        .context()
        .browser()!
        .startTracing(page, {
          categories: [
            "devtools.timeline",
            "disabled-by-default-devtools.timeline",
            "blink.user_timing",
          ],
        });
    }
    if (profiled) {
      await cdp.send("Profiler.enable");
      await cdp.send("Profiler.setSamplingInterval", { interval: 100 });
      await cdp.send("Profiler.start");
    }
    console.log("\n=== 3/4 into the long chapter ===");
    const interactions: Record<string, unknown> = {
      idle: await interact("nothing (baseline)", 4, async () => {}),
      forward: await interact("flip forward", 10, flip(1)),
      back: await interact("flip back", 10, flip(-1)),
      tap: await interact("tap a word", 8, tap),
    };

    if (traced) {
      const to = testInfo.outputPath("interact.trace.json");
      fs.writeFileSync(to, await page.context().browser()!.stopTracing());
      console.log(`trace: ${to}`);
    }
    if (profiled) {
      const { profile } = await cdp.send("Profiler.stop");
      const to = testInfo.outputPath("interact.cpuprofile");
      fs.writeFileSync(to, JSON.stringify(profile));
      console.log(`cpu profile: ${to}`);
    }

    if (cdp && process.env.FLTS_PERF_PROFILE === "open") {
      await remember(false);
      await cdp.send("Profiler.enable");
      await cdp.send("Profiler.setSamplingInterval", { interval: 200 });
      await settleAt(LONG_CHAPTER, longIds[Math.floor(longIds.length * 0.75)]);
      await page.goto(`/book/${bookId}/0`);
      await expect(
        page.locator(".paragraphs-container.is-ready"),
      ).toBeVisible();
      await page.getByTestId("chapters-panel-handle").click();
      const link = page
        .getByTestId("chapter-row")
        .nth(LONG_CHAPTER)
        .locator("a");
      await expect(link).toBeVisible();
      await page.waitForTimeout(400);
      await cdp.send("Profiler.start");
      await measureClick(page, cdp, link);
      const { profile } = await cdp.send("Profiler.stop");
      const out = testInfo.outputPath("chapter-open.cpuprofile");
      fs.writeFileSync(out, JSON.stringify(profile));
      console.log(`cpu profile: ${out}`);
    }

    if (cdp && process.env.FLTS_PERF_TRACE === "open") {
      await remember(false);
      const browser = page.context().browser()!;
      await settleAt(LONG_CHAPTER, longIds[Math.floor(longIds.length * 0.75)]);
      await page.goto(`/book/${bookId}/0`);
      await expect(
        page.locator(".paragraphs-container.is-ready"),
      ).toBeVisible();
      await page.getByTestId("chapters-panel-handle").click();
      const link = page
        .getByTestId("chapter-row")
        .nth(LONG_CHAPTER)
        .locator("a");
      await expect(link).toBeVisible();
      await page.waitForTimeout(400);
      await page.evaluate(() => ((window as any).__perf.probe = false));
      await browser.startTracing(page, {
        categories: [
          "devtools.timeline",
          "disabled-by-default-devtools.timeline",
          "disabled-by-default-devtools.timeline.stack",
          "disabled-by-default-devtools.timeline.invalidationTracking",
          "blink.user_timing",
        ],
      });
      await page.evaluate(() => performance.mark("perf-click"));
      const run = await measureClick(page, cdp, link);
      const trace = await browser.stopTracing();
      await page.evaluate(() => ((window as any).__perf.probe = true));
      const tracePath = testInfo.outputPath("chapter-open.trace.json");
      fs.writeFileSync(tracePath, trace);
      console.log(
        `trace: ${tracePath} (visible at ${run.milestones.visible?.toFixed(0)} ms)`,
      );
    }

    const out = path.join(
      process.env.FLTS_PERF_OUT ?? testInfo.outputDir,
      "chapter-open-perf.json",
    );
    fs.mkdirSync(path.dirname(out), { recursive: true });
    fs.writeFileSync(out, JSON.stringify({ ...report, interactions }));
    console.log(`report: ${out}`);
  });
  // The engine the app ships with: its own window, its own IPC. Needs a
  // display; the window shows while it runs.
  test("a long translated chapter, in the app's own webview", async ({
    harness,
  }, testInfo) => {
    test.skip(
      !process.env.FLTS_PERF_APP || testInfo.project.name !== "real",
      "run with FLTS_PERF_APP=1",
    );
    test.setTimeout(1_200_000);
    const { bookId, longIds } = await seedLongBook(harness);
    const shortIds = await harness.invoke<number[]>(
      "get_book_chapter_paragraph_ids",
      { bookId, chapterId: 0 },
    );
    await harness.restartApp({
      env: { FLTS_E2E_WINDOW_URL: "http://localhost:5183" },
    });

    const pause = (ms: number) => new Promise((r) => setTimeout(r, ms));
    /** Runs `fn` in the app's webview. */
    const inApp = <T, A = null>(fn: (arg: A) => T | Promise<T>, arg?: A) =>
      harness.invoke<T>("e2e_eval", {
        js: `return (${fn.toString()})(${JSON.stringify(arg ?? null)});`,
      });
    const until = async (what: string, fn: () => boolean, ms = 30_000) => {
      const deadline = Date.now() + ms;
      for (;;) {
        // A page between two documents answers nothing.
        if (await inApp(fn).catch(() => false)) return;
        if (Date.now() > deadline) throw new Error(`timed out: ${what}`);
        await pause(100);
      }
    };
    /** A full load of `path`, with the probes installed in the new page. */
    const load = async (path: string, forget: boolean) => {
      await inApp(
        ({ path, forget }) => {
          if (forget) localStorage.removeItem("flts.chapterLayout.v2");
          (window as any).__leaving = true;
          setTimeout(() => (location.href = path), 30);
        },
        { path, forget },
      );
      await until(
        `load ${path}`,
        () => !(window as any).__leaving && document.readyState === "complete",
      );
      await harness.invoke("e2e_eval", {
        js: `if (!window.__perf) (${instrument.toString()})(); return true;`,
      });
    };
    const settle = async (chapterId: number, paragraphId: number) => {
      await load("/", false);
      await pause(600);
      await harness.invoke("save_book_reading_state", {
        bookId,
        chapterId,
        paragraphId,
        pageOffset: 0,
      });
    };
    const collect = async (): Promise<Run> => {
      await until("the chapter with its words", () => {
        const f = (window as any).__perf.frames;
        const last = f[f.length - 1];
        return !!last && last.ready && last.words;
      });
      await pause(1500);
      const raw = await inApp(() => {
        const p = (window as any).__perf;
        p.armed = false;
        return {
          frames: p.frames as Array<Record<string, number | boolean>>,
          revealed: p.revealed as number | null,
          invokes: [] as Array<Record<string, number | string>>,
          events: [] as Array<{ t: number; name: string }>,
          longTasks: [] as Array<{ t: number; ms: number }>,
        };
      });
      return {
        ...raw,
        milestones: milestonesOf(raw),
        cpu: Object.fromEntries(CPU_KEYS.map((k) => [k, NaN])),
      };
    };
    const viaPanel = async (forget: boolean): Promise<Run> => {
      await load(`/book/${bookId}/0`, forget);
      await until(
        "the short chapter",
        () => !!document.querySelector(".paragraphs-container.is-ready"),
      );
      await pause(800);
      await inApp(() =>
        (
          document.querySelector(
            '[data-testid="chapters-panel-handle"]',
          ) as HTMLElement
        ).click(),
      );
      await until(
        "the chapters panel",
        () =>
          document.querySelectorAll('[data-testid="chapter-row"] a').length > 1,
      );
      await pause(400);
      await inApp((chapter) => {
        const link = document.querySelectorAll<HTMLElement>(
          '[data-testid="chapter-row"] a',
        )[chapter];
        link.setAttribute("data-perf-arm", "");
        link.click();
      }, LONG_CHAPTER);
      return collect();
    };
    const deep = longIds[Math.floor(longIds.length * 0.75)];
    const scenarios: Array<[string, (forget: boolean) => Promise<Run>]> = [
      [
        "switch chapter, from its start",
        async (forget) => {
          await settle(0, shortIds[0]);
          return viaPanel(forget);
        },
      ],
      [
        "switch chapter, restoring a position 3/4 in",
        async (forget) => {
          await settle(LONG_CHAPTER, deep);
          return viaPanel(forget);
        },
      ],
      [
        "open from the library, restoring a position 3/4 in",
        async (forget) => {
          await settle(LONG_CHAPTER, deep);
          await load("/", forget);
          await until(
            "the library",
            () => !!document.querySelector('a[href^="/book/"]'),
          );
          await pause(800);
          await inApp((href) => {
            const link = document.querySelector<HTMLElement>(
              `a[href="${href}"]`,
            )!;
            link.setAttribute("data-perf-arm", "");
            link.click();
          }, `/book/${bookId}`);
          return collect();
        },
      ],
    ];

    await until(
      "the app's first page",
      () => document.readyState === "complete",
    );
    console.log(await inApp(() => navigator.userAgent));
    const report: Record<string, unknown> = {};
    for (const forget of [true, false]) {
      for (const [name, run] of scenarios) {
        const title = `${name} — ${forget ? "first time" : "again"}`;
        const runs: Run[] = [];
        await run(forget);
        for (let i = 0; i < ITERATIONS; i++) runs.push(await run(forget));
        report[title] = runs;
        const reveal = runs.map((r) => r.milestones.revealAsked ?? NaN);
        console.log(
          `=== ${title}\n  reveal asked: median ${median(reveal).toFixed(0)} ms [${reveal.map((x) => x.toFixed(0)).join(", ")}]\n  longest frame (hidden, shown): ${runs.map((r) => `(${longestFrames(r).join(", ")})`).join(" ")}`,
        );
      }
    }

    // 3/4 into the long chapter: flips and taps.
    const longest = async (
      label: string,
      times: number,
      action: (i: number) => Promise<unknown>,
    ) => {
      const gaps: number[] = [];
      for (let i = 0; i < times; i++) {
        await inApp(() => {
          const gap = { last: performance.now(), max: 0, on: true };
          (window as any).__gap = gap;
          const tick = () => {
            if (!gap.on) return;
            const now = performance.now();
            gap.max = Math.max(gap.max, now - gap.last);
            gap.last = now;
            requestAnimationFrame(tick);
          };
          requestAnimationFrame(tick);
        });
        await action(i);
        await pause(600);
        gaps.push(
          await inApp(() => {
            const gap = (window as any).__gap;
            gap.on = false;
            return gap.max as number;
          }),
        );
      }
      console.log(
        `  ${label.padEnd(22)} longest frame: median ${median(gaps).toFixed(0)} ms, worst ${Math.max(...gaps).toFixed(0)} ms`,
      );
      return gaps;
    };
    const flip = (pages: number) => () =>
      inApp((n) => {
        const c = document.querySelector(".paragraphs-container")!;
        c.scrollBy({ left: n * c.clientWidth, behavior: "smooth" });
      }, pages);
    if (process.env.FLTS_PERF_PROBE) {
      console.log(
        "probe",
        JSON.stringify(
          await inApp(async () => {
            const c = document.querySelector(
              ".paragraphs-container",
            ) as HTMLElement;
            const med = (xs: number[]) =>
              xs.sort((a, b) => a - b)[xs.length >> 1];
            const timeLayout = (dirty: () => () => void) => {
              const xs: number[] = [];
              for (let k = 0; k < 9; k++) {
                const t = performance.now();
                const undo = dirty();
                void c.scrollWidth;
                xs.push(performance.now() - t);
                undo();
                void c.scrollWidth;
              }
              return +med(xs).toFixed(1);
            };
            const word = [...c.querySelectorAll<HTMLElement>(".word-span")][40];
            const text = c.querySelectorAll(
              ".paragraph-wrapper:not(.far)",
            ).length;
            const far = c.querySelectorAll(".paragraph-wrapper.far").length;
            const layout = timeLayout(() => {
              const s = document.createElement("span");
              s.style.cssText = "position:absolute;top:0;left:0";
              s.textContent = "x";
              word.appendChild(s);
              return () => s.remove();
            });
            // The same with every block gone: what the blocks themselves cost.
            const blocks = [
              ...c.querySelectorAll<HTMLElement>(".paragraph-wrapper.far"),
            ];
            const frames: number[] = [];
            await new Promise<void>((done) => {
              let last = performance.now();
              let n = 0;
              const tick = () => {
                const now = performance.now();
                frames.push(+(now - last).toFixed(0));
                last = now;
                if (++n < 50) requestAnimationFrame(tick);
                else done();
              };
              c.scrollBy({ left: c.clientWidth, behavior: "smooth" });
              requestAnimationFrame(tick);
            });
            return { text, far, blocks: blocks.length, layout, frames };
          }),
        ),
      );
    }
    console.log("\n=== 3/4 into the long chapter ===");
    report.interactions = {
      idle: await longest("nothing (baseline)", 4, async () => {}),
      forward: await longest("flip forward", 10, flip(1)),
      back: await longest("flip back", 10, flip(-1)),
      tap: await longest("tap a word", 8, (i) =>
        inApp((nth) => {
          const c = document.querySelector(".paragraphs-container")!;
          const frame = c.getBoundingClientRect();
          const words = [
            ...c.querySelectorAll<HTMLElement>(".word-span"),
          ].filter((w) => {
            const r = w.getBoundingClientRect();
            return r.left > frame.left && r.right < frame.right;
          });
          words[5 + nth * 7]?.click();
        }, i),
      ),
    };

    const out = path.join(
      process.env.FLTS_PERF_OUT ?? testInfo.outputDir,
      "chapter-open-perf-app.json",
    );
    fs.mkdirSync(path.dirname(out), { recursive: true });
    fs.writeFileSync(out, JSON.stringify(report));
    console.log(`report: ${out}`);
  });
});
