import type { Library } from "../data/library";
import type { UUID } from "../data/uuid";
import type { ChapterParagraphsStore } from "./ChapterParagraphsStore.svelte";

export type WordSelection = {
  paragraphId: number;
  sentence: number;
  word: number;
};

export type ParagraphVMProps = {
  bookId: UUID;
  paragraphId: number;
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
  // Known to have no translation; not merely not asked about yet.
  isUntranslated = $derived(this.#translation?.html === null);
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
