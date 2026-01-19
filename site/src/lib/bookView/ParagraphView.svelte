<script lang="ts">
    import Fa from "svelte-fa";
    import { faLanguage } from "@fortawesome/free-solid-svg-icons";
    import type { ParagraphView } from "../data/sql/book";
    import { getContext, onDestroy, onMount, tick } from "svelte";
    import type { Library, TranslationStatus } from "../data/library";
    import type { UUID } from "../data/v2/db";
    import { listen, type UnlistenFn } from "@tauri-apps/api/event";
    import CircularProgress from "../widgets/CircularProgress.svelte";

    let {
        bookId,
        paragraph,
        sentenceWordIdToDisplay,
    }: {
        bookId: UUID;
        paragraph: ParagraphView;
        sentenceWordIdToDisplay: [number, number, number] | null;
    } = $props();

    const originalText = $derived(paragraph.original);
    const translationHtml = $derived(paragraph.translation);

    const library: Library = getContext("library");

    let translationRequestId: number | null = $state(null);

    let progressChars = $state(0);
    let expectedChars = $state(100);

    let unsub: UnlistenFn | null = null;

    listen<TranslationStatus>("translation_status", (event) => {
        const status = event.payload;
        if (status.request_id === translationRequestId) {
            if (status.is_complete) {
                translationRequestId = null;
                progressChars = 0;
            } else {
                progressChars = status.progress_chars;
                expectedChars = status.expected_chars;
            }
        }
    }).then((u) => {
        unsub = u;
    });

    onDestroy(() => {
        if (unsub) {
            unsub();
        }
    });

    const isTranslating = $derived(translationRequestId !== null);

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
        const selectedElements = document.querySelectorAll(
            ".word-span.selected",
        );
        selectedElements.forEach((el) => {
            el.classList.remove("selected");
        });
        if (sentenceWordIdToDisplay) {
            let element = document.querySelector<HTMLElement>(
                `.word-span[data-paragraph="${sentenceWordIdToDisplay[0]}"][data-sentence="${sentenceWordIdToDisplay[1]}"][data-word="${sentenceWordIdToDisplay[2]}"]`,
            );
            if (element) {
                element.classList.add("selected");
                element.classList.add("show-translation");
                void tick().then(() => shrinkTranslationToFit(element));
            }
        }

        library
            .getParagraphTranslationRequestId(bookId, paragraph.id)
            .then((id) => {
                translationRequestId = id;
                if (id !== null) {
                    progressChars = 0;
                }
            });

        void adjustVisiblePopups();
    });

    $effect(() => {
        // Re-run whenever translation HTML changes to keep overlays sized correctly.
        translationHtml;
        void adjustVisiblePopups();
        void restoreVisibleWords();
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

    function shrinkTranslationToFit(span: HTMLElement) {
        const translationEl =
            span.querySelector<HTMLElement>(".word-translation");
        if (!translationEl) {
            return;
        }

        translationEl.style.fontSize = "";
        const parentWidth = span.getBoundingClientRect().width;
        if (!parentWidth) {
            return;
        }

        const styles = getComputedStyle(translationEl);
        const paddingLeft = parseFloat(styles.paddingLeft) || 0;
        const paddingRight = parseFloat(styles.paddingRight) || 0;
        const borderLeft = parseFloat(styles.borderLeftWidth) || 0;
        const borderRight = parseFloat(styles.borderRightWidth) || 0;
        const horizontalChrome =
            paddingLeft + paddingRight + borderLeft + borderRight;
        const availableWidth = parentWidth - horizontalChrome;
        if (availableWidth <= 0) {
            return;
        }

        const rawContentWidth =
            translationEl.scrollWidth - (paddingLeft + paddingRight);
        if (rawContentWidth <= availableWidth) {
            return;
        }

        const baseFontSize = parseFloat(styles.fontSize);
        if (!baseFontSize || Number.isNaN(baseFontSize)) {
            return;
        }

        const scaledSize = baseFontSize * (availableWidth / rawContentWidth);
        translationEl.style.fontSize = `${scaledSize}px`;
    }

    async function adjustVisiblePopups() {
        await tick();
        const wrapper = document.querySelector<HTMLElement>(
            `.paragraph-wrapper[data-paragraph-id="${paragraph.id}"]`,
        );
        if (!wrapper) {
            return;
        }
        wrapper
            .querySelectorAll<HTMLElement>(".word-span.show-translation")
            .forEach((span) => shrinkTranslationToFit(span));
    }

    /** Restore show-translation class for words that were previously marked visible */
    async function restoreVisibleWords() {
        await tick();
        const wrapper = document.querySelector<HTMLElement>(
            `.paragraph-wrapper[data-paragraph-id="${paragraph.id}"]`,
        );
        if (
            !wrapper ||
            !paragraph.visibleWords ||
            paragraph.visibleWords.length === 0
        ) {
            return;
        }

        const visibleSet = new Set(paragraph.visibleWords);
        const wordSpans = wrapper.querySelectorAll<HTMLElement>(".word-span");

        wordSpans.forEach((span) => {
            const flatIndex = parseInt(span.dataset["flatIndex"] ?? "-1");
            if (visibleSet.has(flatIndex)) {
                span.classList.add("show-translation");
                shrinkTranslationToFit(span);
            }
        });
    }

    onMount(() => {
        void adjustVisiblePopups();
        void restoreVisibleWords();
    });
</script>

<div class="paragraph-wrapper" data-paragraph-id={paragraph.id}>
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

    :global(.word-span .word-translation) {
        display: none;
        position: absolute;
        left: 0;
        right: 0;
        top: 0;
        width: 100%;
        font-size: 0.55em;
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

    :global(.word-span.show-translation .word-translation) {
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
