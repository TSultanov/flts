// Shared real-tier spec helpers: seeding books through the real pipeline and
// reading what the backend stored.

import type { RealHarness } from "./fixtures";

export const MODEL = "models/gemini-2.5-flash";
const SRC = "deu";
/** Streaming translation traffic only; summaries are unary `:generateContent`. */
export const STREAM_GLOB = "*streamGenerateContent*";

/** Unique per call because the disk translation cache outlives the per-test config dir. */
function nonceSeed(): string {
  return `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 8)}`.replace(
    /[^a-z]/g,
    "x",
  );
}

const LETTERS = "abcdefgh";

/** `n` paragraphs of `per` pairwise-distinct nonce lemmas. */
export function lemmaSets(n: number, per = 3): string[][] {
  const seed = nonceSeed();
  return Array.from({ length: n }, (_, i) =>
    Array.from({ length: per }, (_, j) => `w${seed}${LETTERS[i]}${LETTERS[j]}`),
  );
}

export const textOf = (lemmas: string[]) => lemmas.join(" ");

/** Gemini's compact translation schema (library/src/book/translation_import.rs). */
export function translationJson(lemmas: string[]): unknown {
  return {
    s: [
      {
        ft: "full-0",
        wl: lemmas.map((w) => ({
          o: w,
          t: [`t-${w}`],
          n: null,
          p: false,
          g: {
            lf: w,
            lt: `t-${w}`,
            pos: "common_noun",
            pl: null,
            pe: null,
            te: null,
            ca: null,
            ot: null,
          },
        })),
      },
    ],
  };
}

/** One chapter-0 paragraph per lemma set, each with its own scripted answer. */
export async function seedBook(
  h: RealHarness,
  sets: string[][],
  title: string,
): Promise<string> {
  await h.llm.seed({
    scripts: sets.map((lemmas) => ({
      matchSubstring: textOf(lemmas),
      translation: translationJson(lemmas),
    })),
  });
  return h.invoke<string>("import_plain_text", {
    title,
    text: sets.map(textOf).join("\n"),
    sourceLanguageId: SRC,
  });
}

export type WordSegment = {
  kind: string;
  text: string;
  translation: string | null;
};

export async function storedSegments(
  h: RealHarness,
  bookId: string,
  paragraphId: number,
): Promise<WordSegment[] | null> {
  const rows = await h.invoke<
    Array<{ id: number; segments?: WordSegment[] | null }>
  >("get_paragraph_translations_batch", {
    bookId,
    paragraphIds: [paragraphId],
  });
  return rows.find((r) => r.id === paragraphId)?.segments ?? null;
}

/** Ids whose translation is on disk, ascending. */
export async function storedIds(
  h: RealHarness,
  bookId: string,
  count: number,
): Promise<number[]> {
  const rows = await h.invoke<Array<{ id: number; segments?: unknown }>>(
    "get_paragraph_translations_batch",
    { bookId, paragraphIds: [...Array(count).keys()] },
  );
  return rows
    .filter((r) => r.segments)
    .map((r) => r.id)
    .sort((a, b) => a - b);
}

export const drained = (h: RealHarness) =>
  h.invoke<unknown[]>("list_paragraph_translation_activity");
