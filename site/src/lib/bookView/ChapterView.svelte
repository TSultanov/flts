<script lang="ts">
    import { getContext, onDestroy, onMount, tick } from "svelte";
    import type { UUID } from "../data/v2/db";
    import ParagraphView from "./ParagraphView.svelte";
    import type { Library } from "../data/library";

    let {
        sentenceWordIdToDisplay = $bindable(),
        bookId,
        chapterId,
        initialParagraphId = null,
    }: {
        sentenceWordIdToDisplay: [number, number, number] | null;
        bookId: UUID;
        chapterId: number;
        initialParagraphId?: number | null;
    } = $props();

    const library: Library = getContext("library");
    const paragraphs = $derived(
        library.getBookChapterParagraphs(bookId, chapterId),
    );

    let selectedWordElement: HTMLElement | null = null;
    let selectionEffectSeq = 0;

    $effect(() => {
        const seq = ++selectionEffectSeq;
        if (selectedWordElement) {
            selectedWordElement.classList.remove("selected");
        }

        if (!sentenceWordIdToDisplay) {
            selectedWordElement = null;
            return;
        }

        if (!$paragraphs || $paragraphs.length === 0) {
            selectedWordElement = null;
            return;
        }

        const [paragraphId, sentenceId, wordId] = sentenceWordIdToDisplay;

        void tick().then(() => {
            if (seq !== selectionEffectSeq) {
                return;
            }

            const selector = `.word-span[data-paragraph="${paragraphId}"][data-sentence="${sentenceId}"][data-word="${wordId}"]`;
            const root: ParentNode = paragraphsContainer ?? document;
            const element = root.querySelector<HTMLElement>(selector);
            if (!element) {
                selectedWordElement = null;
                return;
            }

            element.classList.add("selected");
            element.classList.add("show-translation");
            selectedWordElement = element;
            shrinkTranslationToFit(element);
        });
    });

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

    function chapterClick(e: MouseEvent) {
        const target = document.elementFromPoint(
            e.clientX,
            e.clientY,
        ) as HTMLElement;
        if (target && target.classList.contains("word-span")) {
            const paragraph = target.dataset["paragraph"]
                ? parseInt(target.dataset["paragraph"])
                : null;
            const sentence = target.dataset["sentence"]
                ? parseInt(target.dataset["sentence"])
                : null;
            const word = target.dataset["word"]
                ? parseInt(target.dataset["word"])
                : null;
            const flatIndex = target.dataset["flatIndex"]
                ? parseInt(target.dataset["flatIndex"])
                : null;

            sentenceWordIdToDisplay =
                paragraph != null && sentence != null && word != null
                    ? [paragraph, sentence, word]
                    : null;

            // Persist word visibility
            if (paragraph != null && flatIndex != null) {
                library
                    .markWordVisible(bookId, paragraph, flatIndex)
                    .catch((err) =>
                        console.error("Failed to mark word visible", err),
                    );
            }
        } else {
            sentenceWordIdToDisplay = null;
        }
    }

    let sectionContentWidth = $state(200);
    let paragraphsContainer: HTMLDivElement | null = null;
    let visibleParagraphId: number | null = null;
    let saveTimeout: ReturnType<typeof setTimeout> | null = null;
    let lastSavedParagraph: number | null = null;
    let initialScrollApplied = false;
    let isResizing = false;
    let resizeTimeout: ReturnType<typeof setTimeout> | null = null;
    let scrollRaf: number | null = null;

    function handleScroll() {
        if (isResizing) {
            return;
        }
        if (scrollRaf !== null) {
            return;
        }
        scrollRaf = requestAnimationFrame(() => {
            scrollRaf = null;
            updateVisibleParagraph();
        });
    }

    function scheduleSave(paragraphId: number) {
        if (saveTimeout) {
            clearTimeout(saveTimeout);
        }

        saveTimeout = setTimeout(() => {
            if (lastSavedParagraph === paragraphId) {
                return;
            }
            lastSavedParagraph = paragraphId;
            library
                .saveBookReadingState(bookId, chapterId, paragraphId)
                .catch((err) =>
                    console.error("Failed to save reading state", err),
                );
        }, 400);
    }

    function updateVisibleParagraph() {
        const nextParagraph = findVisibleParagraph();
        if (nextParagraph == null || nextParagraph === visibleParagraphId) {
            return;
        }
        visibleParagraphId = nextParagraph;
        scheduleSave(nextParagraph);
    }

    function findVisibleParagraph(): number | null {
        if (!paragraphsContainer) {
            return null;
        }
        const containerRect = paragraphsContainer.getBoundingClientRect();
        const x = containerRect.left + 16;
        const y = containerRect.top + containerRect.height / 2;
        const hit = document.elementFromPoint(x, y) as HTMLElement | null;
        const wrapper = hit?.closest<HTMLElement>(".paragraph-wrapper") ?? null;
        const idAttr = wrapper?.dataset["paragraphId"];
        if (!idAttr) {
            return null;
        }
        const id = parseInt(idAttr, 10);
        return Number.isNaN(id) ? null : id;
    }

    async function syncInitialParagraph() {
        await tick();
        if (!paragraphsContainer) {
            return;
        }

        if (!initialScrollApplied && initialParagraphId != null) {
            const target = paragraphsContainer.querySelector<HTMLElement>(
                `[data-paragraph-id="${initialParagraphId}"]`,
            );
            if (target) {
                target.scrollIntoView({
                    behavior: "auto",
                    block: "center",
                    inline: "center",
                });
                visibleParagraphId = initialParagraphId;
                scheduleSave(initialParagraphId);
                initialScrollApplied = true;
                return;
            }
        }

        initialScrollApplied = true;
        updateVisibleParagraph();
    }

    $effect(() => {
        if ($paragraphs && $paragraphs.length > 0) {
            void syncInitialParagraph();
        }
    });

    onMount(() => {
        const listener = () => {
            isResizing = true;
            if (resizeTimeout) {
                clearTimeout(resizeTimeout);
            }

            if (visibleParagraphId != null && paragraphsContainer) {
                const target = paragraphsContainer.querySelector<HTMLElement>(
                    `[data-paragraph-id="${visibleParagraphId}"]`,
                );
                if (target) {
                    target.scrollIntoView({
                        behavior: "auto",
                        block: "center",
                        inline: "center",
                    });
                }
            }

            resizeTimeout = setTimeout(() => {
                isResizing = false;
            }, 200);
        };
        window.addEventListener("resize", listener);
        return () => {
            window.removeEventListener("resize", listener);
            if (resizeTimeout) {
                clearTimeout(resizeTimeout);
            }
        };
    });

    onDestroy(() => {
        if (scrollRaf !== null) {
            cancelAnimationFrame(scrollRaf);
            scrollRaf = null;
        }
        if (saveTimeout) {
            clearTimeout(saveTimeout);
        }
        if (
            visibleParagraphId != null &&
            lastSavedParagraph !== visibleParagraphId
        ) {
            library
                .saveBookReadingState(bookId, chapterId, visibleParagraphId)
                .catch((err) =>
                    console.error("Failed to save reading state", err),
                );
        }
    });
</script>

<div class="chapter-container">
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <section class="chapter" onclick={chapterClick}>
        <div
            class="paragraphs-container"
            style="column-width: {sectionContentWidth}px"
            bind:clientHeight={sectionContentWidth}
            bind:this={paragraphsContainer}
            onscroll={handleScroll}
        >
            {#each $paragraphs as paragraph (paragraph.id)}
                <ParagraphView {bookId} {paragraph} />
            {/each}
        </div>
    </section>
</div>

<style>
    .chapter-container {
        background-color: var(--hover-color);
        padding: 10px 25px;
        justify-content: center;
        height: 100%;
        overflow: hidden;
    }

    .chapter {
        padding: 1cm 0;
        max-width: 800px;
        margin: 0 auto;
        border: 1px solid var(--background-color);
        background-color: white;
        box-shadow: 2px 2px var(--background-color);
        text-align: justify;
        line-height: 2;
        height: 100%;
    }

    .paragraphs-container {
        width: 100%;
        height: 100%;
        overflow-x: auto;
        scroll-snap-type: x mandatory;
        column-gap: 0;
    }

    :global(.paragraphs-container > *) {
        scroll-snap-align: center;
        scroll-snap-stop: always;
    }
</style>
