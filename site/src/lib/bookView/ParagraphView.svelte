<script lang="ts">
    import Fa from "svelte-fa";
    import { faLanguage } from "@fortawesome/free-solid-svg-icons";
    import type { ParagraphView } from "../data/sql/book";
    import { getContext, onMount, tick } from "svelte";
    import type { Library, TranslationStatus } from "../data/library";
    import type { UUID } from "../data/v2/db";
    import CircularProgress from "../widgets/CircularProgress.svelte";

    let {
        bookId,
        paragraph,
    }: {
        bookId: UUID;
        paragraph: ParagraphView;
    } = $props();

    const originalText = $derived(paragraph.original);
    const translationHtml = $derived(paragraph.translation);

    const library: Library = getContext("library");

    let translationRequestId: number | null = $state(null);

    let progressChars = $state(0);
    let expectedChars = $state(100);

    const translationStatus = $derived(
        library.getTranslationStatus(translationRequestId),
    );

    $effect(() => {
        const status: TranslationStatus | undefined = $translationStatus;
        if (!status) {
            return;
        }
        if (status.is_complete) {
            translationRequestId = null;
            progressChars = 0;
            return;
        }

        progressChars = status.progress_chars;
        expectedChars = status.expected_chars;
    });

    const isTranslating = $derived(translationRequestId !== null);

    let translationRequestSyncSeq = 0;

    $effect(() => {
        const currentId = translationRequestId;
        const seq = ++translationRequestSyncSeq;

        if (translationHtml) {
            if (currentId !== null) {
                translationRequestId = null;
            }
            progressChars = 0;
            return;
        }

        if (currentId !== null) {
            return;
        }

        library
            .getParagraphTranslationRequestId(bookId, paragraph.id)
            .then((id) => {
                if (seq !== translationRequestSyncSeq) {
                    return;
                }
                translationRequestId = id;
                if (id !== null) {
                    progressChars = 0;
                }
            })
            .catch(() => {});
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
