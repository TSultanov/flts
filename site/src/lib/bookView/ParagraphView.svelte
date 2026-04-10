<script lang="ts">
    import Fa from "svelte-fa";
    import { faLanguage } from "@fortawesome/free-solid-svg-icons";
    import type { ParagraphView } from "../data/sql/book";
    import { getContext, tick } from "svelte";
    import type { Library, TranslationStatus } from "../data/library";
    import type { UUID } from "../data/v2/db";
    import CircularProgress from "../widgets/CircularProgress.svelte";
    import {
        showTranslation,
        showTranslations,
        showTranslationsBatched,
    } from "./translationOverlay";

    let {
        bookId,
        paragraph,
        sentenceWordIdToDisplay = null,
    }: {
        bookId: UUID;
        paragraph: ParagraphView;
        sentenceWordIdToDisplay?: [number, number, number] | null;
    } = $props();

    let paragraphOverride = $state<ParagraphView | null>(null);
    const effectiveParagraph = $derived(
        paragraphOverride && paragraphOverride.id === paragraph.id
            ? paragraphOverride
            : paragraph,
    );

    const originalText = $derived(effectiveParagraph.original);
    const translationHtml = $derived(effectiveParagraph.translation);
    const visibleWords = $derived(effectiveParagraph.visibleWords);

    const library: Library = getContext("library");

    let translationRequestId: number | null = $state(null);

    let progressChars = $state(0);
    let expectedChars = $state(100);
    let wrapper: HTMLDivElement | null = $state(null);
    let shouldRestoreVisibleWords = $state(false);
    let visibleWordsRestored = $state(false);

    const translationStatus = $derived(
        library.getTranslationStatus(translationRequestId),
    );

    $effect(() => {
        const status: TranslationStatus | undefined = $translationStatus;
        if (!status) {
            return;
        }
        if (status.is_complete) {
            if (status.error) {
                console.warn(`Translation failed for paragraph ${paragraph.id}:`, status.error);
            }
            void refreshParagraphView();
            translationRequestId = null;
            progressChars = 0;
            return;
        }

        progressChars = status.progress_chars;
        expectedChars = status.expected_chars;
    });

    const isTranslating = $derived(translationRequestId !== null);

    let paragraphRefreshSeq = 0;

    async function refreshParagraphView() {
        const seq = ++paragraphRefreshSeq;
        try {
            const updated = await library.getParagraphView(bookId, paragraph.id);
            if (seq !== paragraphRefreshSeq) {
                return;
            }
            paragraphOverride = updated;
        } catch {
        }
    }

    $effect(() => {
        if (translationHtml) {
            if (translationRequestId !== null) {
                translationRequestId = null;
            }
            progressChars = 0;
            return;
        }

        if (translationRequestId !== null) {
            return;
        }

        let cancelled = false;
        library
            .getParagraphTranslationRequestId(bookId, paragraph.id)
            .then((id) => {
                if (cancelled) {
                    return;
                }
                translationRequestId = id;
                if (id !== null) {
                    progressChars = 0;
                }
            })
            .catch(() => {});

        return () => {
            cancelled = true;
        };
    });

    $effect(() => {
        if (translationRequestId === null) {
            return;
        }

        let cancelled = false;
        const interval = setInterval(async () => {
            if (cancelled) {
                return;
            }
            try {
                const id = await library.getParagraphTranslationRequestId(
                    bookId,
                    paragraph.id,
                );
                if (cancelled) {
                    return;
                }
                if (id === null) {
                    void refreshParagraphView();
                    translationRequestId = null;
                    progressChars = 0;
                }
            } catch {
            }
        }, 1000);

        return () => {
            cancelled = true;
            clearInterval(interval);
        };
    });

    $effect(() => {
        // Re-run whenever translation HTML changes to keep overlays sized correctly.
        translationHtml;
        visibleWords;
        wrapper;

        visibleWordsRestored = false;
    });

    $effect(() => {
        if (!wrapper) {
            shouldRestoreVisibleWords = false;
            return;
        }

        const root =
            wrapper.closest<HTMLElement>(".paragraphs-container") ?? null;
        if (!root || !("IntersectionObserver" in window)) {
            shouldRestoreVisibleWords = true;
            return;
        }

        const observer = new IntersectionObserver(
            ([entry]) => {
                shouldRestoreVisibleWords = !!entry?.isIntersecting;
            },
            { root, threshold: 0.01 },
        );

        observer.observe(wrapper);
        return () => observer.disconnect();
    });

    $effect(() => {
        if (
            visibleWordsRestored ||
            !shouldRestoreVisibleWords ||
            !wrapper ||
            !translationHtml ||
            visibleWords.length === 0
        ) {
            return;
        }

        const controller = new AbortController();
        void restoreVisibleWords(controller.signal).then(() => {
            if (!controller.signal.aborted) {
                visibleWordsRestored = true;
            }
        });

        return () => controller.abort();
    });

    $effect(() => {
        if (!wrapper || !translationHtml || !sentenceWordIdToDisplay) {
            return;
        }

        const [paragraphId, sentenceId, wordId] = sentenceWordIdToDisplay;
        if (paragraphId !== paragraph.id) {
            return;
        }

        let cancelled = false;
        let selected: HTMLElement | null = null;
        void tick().then(() => {
            if (cancelled) {
                return;
            }
            if (!wrapper) {
                return;
            }

            const element = wrapper.querySelector<HTMLElement>(
                `.word-span[data-sentence="${sentenceId}"][data-word="${wordId}"]`,
            );
            if (!element) {
                return;
            }

            element.classList.add("selected");
            showTranslation(element);
            selected = element;
        });

        return () => {
            cancelled = true;
            selected?.classList.remove("selected");
        };
    });

    async function translateParagraph(event: MouseEvent) {
        const useCache = !(event.metaKey || event.ctrlKey);

        progressChars = 0;
        translationRequestId = await library.translateParagraph(
            bookId,
            paragraph.id,
            undefined,
            useCache,
        );
    }

    /** Restore show-translation class for words that were previously marked visible */
    async function restoreVisibleWords(signal?: AbortSignal) {
        await tick();
        if (signal?.aborted) {
            return;
        }

        if (
            !wrapper ||
            visibleWords.length === 0
        ) {
            return;
        }

        const spans: HTMLElement[] = [];
        if (visibleWords.length > 50) {
            const spanByFlatIndex = new Map<number, HTMLElement>();
            wrapper.querySelectorAll<HTMLElement>(".word-span").forEach((span) => {
                const flatIndex = parseInt(span.dataset["flatIndex"] ?? "", 10);
                if (!Number.isNaN(flatIndex)) {
                    spanByFlatIndex.set(flatIndex, span);
                }
            });
            for (const flatIndex of visibleWords) {
                const span = spanByFlatIndex.get(flatIndex);
                if (span) {
                    spans.push(span);
                }
            }
        } else {
            for (const flatIndex of visibleWords) {
                const span = wrapper.querySelector<HTMLElement>(
                    `.word-span[data-flat-index="${flatIndex}"]`,
                );
                if (span) {
                    spans.push(span);
                }
            }
        }
        if (signal?.aborted) {
            return;
        }

        if (spans.length > 200) {
            await showTranslationsBatched(spans, { signal, batchSize: 200 });
            return;
        }
        showTranslations(spans);
    }
</script>

<div
    class="paragraph-wrapper"
    data-paragraph-id={paragraph.id}
    bind:this={wrapper}
>
    {#if !translationHtml}
        <button
            class="translate"
            aria-label="Translate paragraph"
            title="Translate paragraph"
            onclick={translateParagraph}
            disabled={isTranslating}
        >
            {#if isTranslating}
                <CircularProgress
                    value={progressChars}
                    max={expectedChars}
                    size="1.2em"
                    strokeWidth={4}
                />
            {:else}
                <Fa icon={faLanguage} />
            {/if}
        </button>
        <p class="original">
            {@html originalText}
        </p>
    {:else}
        <div></div>
        <p>
            {@html translationHtml}
        </p>
    {/if}
</div>

<style>
    :global(.word-span.selected) {
        outline: 1px dotted var(--selected-color);
    }

    :global(.word-span) {
        position: relative;
        display: inline-block;
    }

    :global(.word-span::before) {
        content: attr(data-translation);
        display: none;
        position: absolute;
        left: 0;
        right: 0;
        top: 0;
        width: 100%;
        font-size: var(--word-translation-font-size, 0.55em);
        text-align: center;
        line-height: 1;
        padding: 0.05em 0.1em;
        box-sizing: border-box;
        white-space: nowrap;
        opacity: 0;
        -webkit-user-select: none;
        user-select: none;
        pointer-events: none;
        transition: opacity 150ms ease;
        z-index: 2;
        overflow: hidden;
    }

    :global(.word-span.show-translation[data-translation]::before) {
        display: block;
        opacity: 0.9;
    }

    .original {
        color: var(--text-untranslated);
    }

    p {
        margin: 0;
    }

    .paragraph-wrapper {
        margin-top: 0;
        margin-bottom: 0.5em;
        display: grid;
        grid-template-columns: 1.5cm auto 1.5cm;
        break-inside: avoid;
        -webkit-column-break-inside: avoid;
    }

    /* iOS/WebKit can struggle when forced to keep long blocks unbroken inside columns. */
    @media (pointer: coarse) {
        .paragraph-wrapper {
            break-inside: auto;
            -webkit-column-break-inside: auto;
        }
    }

    button.translate {
        /* margin-top: 0.5em; */
        width: calc(2 * var(--font-size));
        height: calc(2 * var(--font-size));
        padding: 0;
        justify-self: center;
        display: flex;
        align-items: center;
        justify-content: center;
    }
</style>
