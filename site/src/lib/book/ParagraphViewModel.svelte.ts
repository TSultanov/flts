import type { Library, Mark, ParagraphSegment } from "../data/library";
import type { UUID } from "../data/uuid";
import type { ChapterParagraphsStore } from "./ChapterParagraphsStore.svelte";

/**
 * A virtualized paragraph's content. Adjacent segments that share a mark set
 * merge, so the fallback costs a few text nodes instead of one per word. The
 * characters and the wrappers stay the same.
 */
export type SegmentRun =
  | { kind: "text"; text: string; marks?: Mark[] }
  | { kind: "break"; marks?: Mark[] };

function sameMarks(a: Mark[] | undefined, b: Mark[] | undefined): boolean {
  const x = a ?? [];
  const y = b ?? [];
  return x.length === y.length && x.every((m, i) => m === y[i]);
}

export type WordSelection = {
  paragraphId: number;
  sentence: number;
  word: number;
};

export type ParagraphVMProps = {
  bookId: UUID;
  paragraphId: number;
  selection: WordSelection | null;
};

export class ParagraphViewModel {
  #library!: Library;
  #store!: ChapterParagraphsStore;
  #props!: ParagraphVMProps;

  #activity = $derived.by(() =>
    this.#library.getParagraphTranslationActivity(
      this.#props.bookId,
      this.#props.paragraphId,
    ),
  );
  #translation = $derived.by(() =>
    this.#store.getTranslation(this.#props.paragraphId),
  );

  isReady = $derived(this.#store.hasOriginal(this.#props.paragraphId));
  originalText = $derived(
    this.#store.getOriginal(this.#props.paragraphId) ?? "",
  );
  segments = $derived<ParagraphSegment[] | null>(
    this.#translation?.segments ?? null,
  );
  runs = $derived.by<SegmentRun[] | null>(() => {
    const segments = this.segments;
    if (!segments) return null;
    const runs: SegmentRun[] = [];
    for (const seg of segments) {
      if (seg.kind === "break") {
        runs.push({ kind: "break", marks: seg.marks });
        continue;
      }
      const last = runs[runs.length - 1];
      if (last?.kind === "text" && sameMarks(last.marks, seg.marks)) {
        last.text += seg.text;
        continue;
      }
      runs.push({ kind: "text", text: seg.text, marks: seg.marks });
    }
    return runs;
  });
  isTranslating = $derived(this.#activity.current !== null);
  progressChars = $derived(this.#activity.current?.progressChars ?? 0);
  expectedChars = $derived(this.#activity.current?.expectedChars ?? 100);

  constructor(
    library: Library,
    store: ChapterParagraphsStore,
    props: ParagraphVMProps,
  ) {
    this.#library = library;
    this.#store = store;
    this.#props = props;
  }

  isSelected(sentence: number, word: number): boolean {
    const sel = this.#props.selection;
    if (!sel) return false;
    return (
      sel.paragraphId === this.#props.paragraphId &&
      sel.sentence === sentence &&
      sel.word === word
    );
  }

  async translate(useCache: boolean): Promise<void> {
    const { bookId, paragraphId } = this.#props;
    await this.#library.translateParagraph(
      bookId,
      paragraphId,
      undefined,
      useCache,
    );
  }
}
