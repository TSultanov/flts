import { tick } from "svelte";
import type { Library } from "../data/library";
import type { ParagraphView } from "../data/sql/book";
import type { UUID } from "../data/v2/db";
import {
    showTranslation,
    showTranslations,
    showTranslationsBatched,
} from "./translationOverlay";

export type ParagraphVMProps = {
    bookId: UUID;
    paragraph: ParagraphView;
    sentenceWordIdToDisplay: [number, number, number] | null;
};

export class ParagraphViewModel {
    wrapper: HTMLDivElement | null = $state(null);

    progressChars = $state(0);
    expectedChars = $state(100);

    #library: Library;
    #getProps: () => ParagraphVMProps;
    #paragraphOverride = $state<ParagraphView | null>(null);
    #translationRequestId = $state<number | null>(null);
    #refreshSeq = 0;

    #effectiveParagraph = $derived.by(() => {
        const override = this.#paragraphOverride;
        const current = this.#getProps().paragraph;
        return override && override.id === current.id ? override : current;
    });

    #visibleWords = $derived(this.#effectiveParagraph.visibleWords);

    originalText = $derived(this.#effectiveParagraph.original);
    translationHtml = $derived(this.#effectiveParagraph.translation);
    isTranslating = $derived(this.#translationRequestId !== null);

    constructor(library: Library, getProps: () => ParagraphVMProps) {
        this.#library = library;
        this.#getProps = getProps;

        $effect(() => {
            const id = this.#translationRequestId;
            if (id === null) {
                return;
            }
            const store = this.#library.getTranslationStatus(id);
            return store.subscribe((status) => {
                if (!status) return;
                if (status.is_complete) {
                    if (status.error) {
                        console.warn(
                            `Translation failed for paragraph ${this.#getProps().paragraph.id}:`,
                            status.error,
                        );
                    }
                    void this.#refreshParagraphView();
                    this.#translationRequestId = null;
                    this.progressChars = 0;
                    return;
                }
                this.progressChars = status.progress_chars;
                this.expectedChars = status.expected_chars;
            });
        });

        $effect(() => {
            if (this.translationHtml) {
                if (this.#translationRequestId !== null) {
                    this.#translationRequestId = null;
                }
                this.progressChars = 0;
                return;
            }
            if (this.#translationRequestId !== null) {
                return;
            }

            const { bookId, paragraph } = this.#getProps();
            let cancelled = false;
            this.#library
                .getParagraphTranslationRequestId(bookId, paragraph.id)
                .then((id) => {
                    if (cancelled) return;
                    this.#translationRequestId = id;
                    if (id !== null) {
                        this.progressChars = 0;
                    }
                })
                .catch(() => {});
            return () => {
                cancelled = true;
            };
        });

        $effect(() => {
            const wrapper = this.wrapper;
            const hasTranslation = !!this.translationHtml;
            const words = this.#visibleWords;
            if (!wrapper || !hasTranslation || words.length === 0) {
                return;
            }

            let restored = false;
            const controller = new AbortController();
            const run = () => {
                if (restored || controller.signal.aborted) return;
                void this.#restoreVisibleWords(controller.signal).then(() => {
                    if (!controller.signal.aborted) restored = true;
                });
            };

            const root =
                wrapper.closest<HTMLElement>(".paragraphs-container") ?? null;
            if (!root || !("IntersectionObserver" in window)) {
                run();
                return () => controller.abort();
            }
            const observer = new IntersectionObserver(
                ([entry]) => {
                    if (entry?.isIntersecting) run();
                },
                { root, threshold: 0.01 },
            );
            observer.observe(wrapper);
            return () => {
                controller.abort();
                observer.disconnect();
            };
        });

        $effect(() => {
            const target = this.#getProps().sentenceWordIdToDisplay;
            const wrapper = this.wrapper;
            const hasTranslation = !!this.translationHtml;
            if (!wrapper || !hasTranslation || !target) return;
            const [pid, sid, wid] = target;
            if (pid !== this.#getProps().paragraph.id) return;

            let cancelled = false;
            let selected: HTMLElement | null = null;
            void tick().then(() => {
                if (cancelled) return;
                const el = this.wrapper?.querySelector<HTMLElement>(
                    `.word-span[data-sentence="${sid}"][data-word="${wid}"]`,
                );
                if (!el) return;
                el.classList.add("selected");
                showTranslation(el);
                selected = el;
            });
            return () => {
                cancelled = true;
                selected?.classList.remove("selected");
            };
        });
    }

    async translate(useCache: boolean): Promise<void> {
        const { bookId, paragraph } = this.#getProps();
        this.progressChars = 0;
        this.#translationRequestId = await this.#library.translateParagraph(
            bookId,
            paragraph.id,
            undefined,
            useCache,
        );
    }

    async #refreshParagraphView(): Promise<void> {
        const seq = ++this.#refreshSeq;
        const { bookId, paragraph } = this.#getProps();
        try {
            const updated = await this.#library.getParagraphView(
                bookId,
                paragraph.id,
            );
            if (seq !== this.#refreshSeq) return;
            this.#paragraphOverride = updated;
        } catch {
            // ignore — stale request id or transient backend error
        }
    }

    async #restoreVisibleWords(signal?: AbortSignal): Promise<void> {
        await tick();
        if (signal?.aborted) return;

        const wrapper = this.wrapper;
        const words = this.#visibleWords;
        if (!wrapper || words.length === 0) return;

        const spans: HTMLElement[] = [];
        if (words.length > 50) {
            const spanByFlatIndex = new Map<number, HTMLElement>();
            wrapper
                .querySelectorAll<HTMLElement>(".word-span")
                .forEach((span) => {
                    const flatIndex = parseInt(
                        span.dataset["flatIndex"] ?? "",
                        10,
                    );
                    if (!Number.isNaN(flatIndex)) {
                        spanByFlatIndex.set(flatIndex, span);
                    }
                });
            for (const flatIndex of words) {
                const span = spanByFlatIndex.get(flatIndex);
                if (span) spans.push(span);
            }
        } else {
            for (const flatIndex of words) {
                const span = wrapper.querySelector<HTMLElement>(
                    `.word-span[data-flat-index="${flatIndex}"]`,
                );
                if (span) spans.push(span);
            }
        }
        if (signal?.aborted) return;

        if (spans.length > 200) {
            await showTranslationsBatched(spans, { signal, batchSize: 200 });
            return;
        }
        showTranslations(spans);
    }
}
