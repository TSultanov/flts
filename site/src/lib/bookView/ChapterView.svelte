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

    function parseDatasetInt(el: HTMLElement, key: string): number | null {
        const value = el.dataset[key];
        if (!value) {
            return null;
        }
        const parsed = parseInt(value, 10);
        return Number.isNaN(parsed) ? null : parsed;
    }

    function chapterClick(e: MouseEvent) {
        const target = e.target instanceof Element ? e.target : null;
        const wordSpan = target?.closest<HTMLElement>(".word-span") ?? null;
        if (wordSpan) {
            const paragraph = parseDatasetInt(wordSpan, "paragraph");
            const sentence = parseDatasetInt(wordSpan, "sentence");
            const word = parseDatasetInt(wordSpan, "word");
            const flatIndex = parseDatasetInt(wordSpan, "flatIndex");

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
    let isResizing = false;
    let resizeTimeout: ReturnType<typeof setTimeout> | null = null;
    let scrollRaf: number | null = null;
    let initialParagraphSyncedFor: number | null | undefined = undefined;

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
        if (nextParagraph != null) {
            setVisibleParagraph(nextParagraph);
        }
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

    function findParagraphWrapper(paragraphId: number): HTMLElement | null {
        if (!paragraphsContainer) {
            return null;
        }
        const targetId = String(paragraphId);
        const children = paragraphsContainer.children;
        for (let i = 0; i < children.length; i++) {
            const child = children[i] as HTMLElement;
            if (child.dataset["paragraphId"] === targetId) {
                return child;
            }
        }
        return null;
    }

    function setVisibleParagraph(paragraphId: number) {
        if (visibleParagraphId === paragraphId) {
            return;
        }
        visibleParagraphId = paragraphId;
        scheduleSave(paragraphId);
    }

    function scrollParagraphIntoView(
        paragraphId: number,
        options: ScrollIntoViewOptions = {
            behavior: "auto",
            block: "nearest",
            inline: "center",
        },
    ): boolean {
        const target = findParagraphWrapper(paragraphId);
        if (!target) {
            return false;
        }
        target.scrollIntoView(options);
        return true;
    }

    $effect(() => {
        const ps = $paragraphs;

        if (!ps || ps.length === 0) {
            return;
        }

        if (initialParagraphSyncedFor === initialParagraphId) {
            return;
        }

        if (initialParagraphId == null) {
            setVisibleParagraph(ps[0].id);
            initialParagraphSyncedFor = null;
            return;
        }

        if (!paragraphsContainer) {
            return;
        }

        const paragraphIdToScrollTo = initialParagraphId;
        initialParagraphSyncedFor = paragraphIdToScrollTo;
        const controller = new AbortController();

        void (async () => {
            let scrolled = scrollParagraphIntoView(paragraphIdToScrollTo);
            if (!scrolled) {
                await tick();
                if (controller.signal.aborted) {
                    return;
                }
                scrolled = scrollParagraphIntoView(paragraphIdToScrollTo);
            }

            if (controller.signal.aborted) {
                return;
            }

            if (scrolled) {
                setVisibleParagraph(paragraphIdToScrollTo);
            } else if (ps.length > 0) {
                setVisibleParagraph(ps[0].id);
            }
        })();

        return () => controller.abort();
    });

    onMount(() => {
        const listener = () => {
            isResizing = true;
            if (resizeTimeout) {
                clearTimeout(resizeTimeout);
            }

            if (visibleParagraphId != null && paragraphsContainer) {
                scrollParagraphIntoView(visibleParagraphId, {
                    behavior: "auto",
                    block: "center",
                    inline: "center",
                });
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
                <ParagraphView
                    {bookId}
                    {paragraph}
                    {sentenceWordIdToDisplay}
                />
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
        column-fill: auto;
        -webkit-column-fill: auto;
    }

    :global(.paragraphs-container > *) {
        scroll-snap-align: center;
        scroll-snap-stop: always;
    }
</style>
