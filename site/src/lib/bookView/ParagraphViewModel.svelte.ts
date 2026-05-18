import { tick } from "svelte";
import type { Library } from "../data/library";
import type { UUID } from "../data/v2/db";
import {
    showTranslation,
    showTranslations,
    showTranslationsBatched,
} from "./translationOverlay";

export type ParagraphVMProps = {
    bookId: UUID;
    paragraphId: number;
    sentenceWordIdToDisplay: [number, number, number] | null;
};

export class ParagraphViewModel {
    wrapper: HTMLDivElement | null = $state(null);

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

    #visibleWords = $derived(this.#paragraph.current?.visibleWords ?? []);

    originalText = $derived(this.#paragraph.current?.original ?? "");
    translationHtml = $derived(this.#paragraph.current?.translation);
    isTranslating = $derived(this.#activity.current !== null);
    progressChars = $derived(this.#activity.current?.progressChars ?? 0);
    expectedChars = $derived(this.#activity.current?.expectedChars ?? 100);

    constructor(library: Library, props: ParagraphVMProps) {
        this.#library = library;
        this.#props = props;

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
            const target = this.#props.sentenceWordIdToDisplay;
            const wrapper = this.wrapper;
            const hasTranslation = !!this.translationHtml;
            if (!wrapper || !hasTranslation || !target) return;
            const [pid, sid, wid] = target;
            if (pid !== this.#props.paragraphId) return;

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
        const { bookId, paragraphId } = this.#props;
        await this.#library.translateParagraph(
            bookId,
            paragraphId,
            undefined,
            useCache,
        );
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
