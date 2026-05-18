import type { Library } from "../data/library";
import type { ParagraphSegment } from "../data/sql/book";
import type { UUID } from "../data/v2/db";

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
    #props!: ParagraphVMProps;

    #paragraph = $derived.by(() =>
        this.#library.getParagraphView(
            this.#props.bookId,
            this.#props.paragraphId,
        ),
    );
    #activity = $derived.by(() =>
        this.#library.getParagraphTranslationActivity(
            this.#props.bookId,
            this.#props.paragraphId,
        ),
    );

    isReady = $derived(this.#paragraph.current !== undefined);
    originalText = $derived(this.#paragraph.current?.original ?? "");
    segments = $derived<ParagraphSegment[] | null>(
        this.#paragraph.current?.segments ?? null,
    );
    visibleWordsSet = $derived(
        new Set(this.#paragraph.current?.visibleWords ?? []),
    );
    isTranslating = $derived(this.#activity.current !== null);
    progressChars = $derived(this.#activity.current?.progressChars ?? 0);
    expectedChars = $derived(this.#activity.current?.expectedChars ?? 100);

    constructor(library: Library, props: ParagraphVMProps) {
        this.#library = library;
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
