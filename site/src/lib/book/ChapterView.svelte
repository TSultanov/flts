<script lang="ts">
    import Fa from "svelte-fa";
    import { faLanguage } from "@fortawesome/free-solid-svg-icons";
    import { getContext, onDestroy, onMount, setContext } from "svelte";
    import type { UUID } from "../data/uuid";
    import ParagraphView from "./ParagraphView.svelte";
    import { translatedParagraph } from "./translatedParagraph";
    import type { Library } from "../data/library";
    import type { WordSelection } from "./ParagraphViewModel.svelte";
    import {
        ChapterViewModel,
        type WordClickInfo,
    } from "./ChapterViewModel.svelte";
    import { CHAPTER_STORE_KEY } from "./ChapterParagraphsStore.svelte";
    import {
        SUMMARY_STATUS_KEY,
        type BookSummaryStatusStore,
    } from "./BookSummaryStatusStore.svelte";

    let {
        selection = $bindable(),
        bookId,
        chapterId,
        translationRatio = 0,
        initialParagraphId = null,
        initialPageOffset = 0,
        onPositionChange,
    }: {
        selection: WordSelection | null;
        bookId: UUID;
        chapterId: number;
        translationRatio?: number;
        initialParagraphId?: number | null;
        initialPageOffset?: number;
        onPositionChange?: (
            chapterId: number,
            paragraphId: number,
            pageOffset: number,
        ) => void;
    } = $props();

    const library: Library = getContext("library");
    const summaryStatusHolder: { store: BookSummaryStatusStore | null } =
        getContext(SUMMARY_STATUS_KEY);
    const canTranslate = $derived(
        summaryStatusHolder.store?.canTranslate(chapterId) ?? true,
    );
    const showTranslateChapterButton = $derived(translationRatio < 1);

    function handleTranslateChapter() {
        library
            .translateChapter(bookId, chapterId)
            .catch((err) => console.error("Failed to translate chapter", err));
    }

    let paragraphsContainer = $state<HTMLDivElement | null>(null);
    // `column-width` minimum hint: any value ≥ clientWidth forces exactly
    // one visible column, which clientHeight guarantees at normal aspect
    // ratios. One visible column is one page.
    let columnWidthHint = $state(200);
    // Snap targets sit at multiples of the visible column width — one page.
    let containerVisibleWidth = $state(800);

    const vm = new ChapterViewModel(library, {
        get bookId() {
            return bookId;
        },
        get chapterId() {
            return chapterId;
        },
        get initialParagraphId() {
            return initialParagraphId;
        },
        get initialPageOffset() {
            return initialPageOffset;
        },
        get container() {
            return paragraphsContainer;
        },
        get onPositionChange() {
            return onPositionChange;
        },
    });

    setContext(CHAPTER_STORE_KEY, vm.store);

    function handleWordClick(info: WordClickInfo) {
        selection = vm.handleWordClick(info);
    }

    // One handler for every word: they come as HTML, with where they are
    // written on them.
    function handleBackgroundClick(e: MouseEvent) {
        const target = e.target instanceof Element ? e.target : null;
        const span = target?.closest<HTMLElement>(".word-span");
        const wrapper = span?.closest<HTMLElement>(".paragraph-wrapper");
        if (!span || !wrapper) {
            selection = null;
            return;
        }
        handleWordClick({
            paragraphId: Number(wrapper.dataset["paragraphId"]),
            sentence: Number(span.dataset["s"]),
            word: Number(span.dataset["w"]),
            flatIndex: Number(span.dataset["flatIndex"]),
        });
    }

    $effect(() => vm.startInitialSync());

    onMount(() => {
        const listener = () => vm.handleResize();
        window.addEventListener("resize", listener);
        return () => window.removeEventListener("resize", listener);
    });

    onDestroy(() => vm.dispose());
</script>

<div class="chapter-container">
    {#if showTranslateChapterButton}
        <button
            type="button"
            class="translate-chapter"
            data-testid="translate-chapter-button"
            aria-label="Translate all untranslated paragraphs in this chapter"
            title={canTranslate
                ? "Translate all untranslated paragraphs in this chapter"
                : "Waiting for chapter summaries…"}
            disabled={!canTranslate}
            onclick={handleTranslateChapter}
        >
            <Fa icon={faLanguage} />
        </button>
    {/if}
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <section class="chapter" onclick={handleBackgroundClick}>
        <div
            class="paragraphs-container"
            class:is-ready={vm.isInitiallyReady}
            class:snaps={vm.snaps}
            class:is-whole={vm.isWhole}
            data-layout-misses={vm.layoutMisses}
            style="column-width: {columnWidthHint}px"
            bind:clientHeight={columnWidthHint}
            bind:clientWidth={containerVisibleWidth}
            bind:this={paragraphsContainer}
            onscroll={() => vm.handleScroll()}
        >
            {#each Array(vm.columnCount) as _, i (i)}
                <i
                    class="snap-target"
                    style="left: {i * containerVisibleWidth}px"
                    aria-hidden="true"
                ></i>
            {/each}
            {#each vm.drawnParagraphIds as paragraphId (paragraphId)}
                {@const far = vm.farExtent(paragraphId)}
                {#if far}
                    <!-- Forced column ends, not one tall block: a block is
                         moved whole to the next column where a paragraph
                         would have been split. -->
                    <div
                        class="paragraph-wrapper far"
                        class:leads={far.lead}
                        data-paragraph-id={paragraphId}
                    >
                        {#each { length: far.breaks } as _, i (i)}<i
                                class="column-end"
                            ></i>{/each}<i style:height="{far.fill}px"></i>
                    </div>
                {:else if vm.store.getTranslation(paragraphId)?.html != null}
                    <!-- Translated: the backend's HTML, with what only the
                         page knows kept on it. -->
                    <div
                        class="paragraph-wrapper"
                        data-paragraph-id={paragraphId}
                        use:translatedParagraph={{
                            html: vm.store.getTranslation(paragraphId)!.html!,
                            selected:
                                selection?.paragraphId === paragraphId
                                    ? selection
                                    : null,
                            revealed: vm.revealedIn(paragraphId),
                        }}
                    >
                        <p></p>
                    </div>
                {:else if !vm.isMounted(paragraphId)}
                    <!-- What ParagraphView draws for a paragraph that is
                         neither near the reader nor translated, without the
                         component: most of a chapter on its way in. -->
                    <div
                        class="paragraph-wrapper"
                        data-paragraph-id={paragraphId}
                    >
                        <p class="original">
                            {@html vm.store.getOriginal(paragraphId) ||
                                "&nbsp;"}
                        </p>
                    </div>
                {:else}
                    <ParagraphView
                        {bookId}
                        {chapterId}
                        {paragraphId}
                        onReady={() => vm.registerParagraphReady(paragraphId)}
                    />
                {/if}
            {/each}
        </div>
    </section>
</div>

<style>
    .chapter-container {
        position: relative;
        background-color: var(--well-background);
        padding: 10px 25px;
        justify-content: center;
        height: 100%;
        overflow: hidden;
    }

    .translate-chapter {
        position: absolute;
        top: 14px;
        right: 14px;
        z-index: 11;
        width: 36px;
        height: 36px;
        padding: 0;
        border: 1px solid var(--background-color);
        border-radius: 50%;
        background-color: var(--dialog-background);
        color: var(--dialog-text);
        cursor: pointer;
        display: flex;
        align-items: center;
        justify-content: center;
        box-shadow: 2px 2px 4px rgba(0, 0, 0, 0.2);
        font-size: 1.1em;
    }

    .translate-chapter:hover:not(:disabled) {
        background-color: var(--button-cancel-hover);
    }

    .translate-chapter:disabled {
        opacity: 0.5;
        cursor: not-allowed;
    }

    .translate-chapter :global(svg) {
        pointer-events: none;
    }

    @media (max-width: 576px) {
        .translate-chapter {
            top: 8px;
            right: 8px;
        }
    }

    .chapter {
        padding: 1cm 0;
        max-width: 800px;
        margin: 0 auto;
        border: 1px solid var(--background-color);
        background-color: var(--dialog-background);
        box-shadow: 2px 2px var(--background-color);
        text-align: justify;
        line-height: 2;
        height: 100%;
    }

    .paragraphs-container {
        position: relative;
        width: 100%;
        height: 100%;
        overflow-x: auto;
        column-gap: 0;
        column-fill: auto;
        -webkit-column-fill: auto;
        visibility: hidden;
    }

    .paragraphs-container.is-ready {
        visibility: visible;
    }

    .paragraphs-container.snaps {
        scroll-snap-type: x mandatory;
    }

    /* One rule for a paragraph however it is drawn: its room must not
       depend on which of the three ways that is. */
    .paragraphs-container > :global(.paragraph-wrapper) {
        --paragraph-gutter: 1.5cm;
        position: relative;
        margin-top: 0;
        margin-bottom: 0.5em;
        padding-inline: var(--paragraph-gutter);
    }

    .paragraphs-container > :global(.paragraph-wrapper > p) {
        margin: 0;
    }

    .paragraphs-container > :global(.paragraph-wrapper > p.original) {
        color: var(--text-untranslated);
    }

    .paragraphs-container > :global(.paragraph-wrapper.far) {
        margin-bottom: 0;
        padding-inline: 0;
    }

    @media (max-width: 576px) {
        .paragraphs-container > :global(.paragraph-wrapper) {
            --paragraph-gutter: 8mm;
        }
    }

    /* Words come as HTML from the backend, so their looks are set here. */
    .paragraphs-container :global(.word-span) {
        position: relative;
        /* Not inline-block: an atomic per-word box breaks lines differently
           from the same characters as one run. */
        display: inline;
        /* Vertical padding on an inline box does not affect line height. It
           gives the tap target and the overlay anchor without moving text. */
        padding: calc(var(--font-size) * 0.375) 0;
    }

    .paragraphs-container :global(.word-span.selected) {
        outline: 1px dotted var(--selected-color);
    }

    .paragraphs-container :global(.translation-overlay) {
        position: absolute;
        left: 0;
        right: 0;
        /* Top of the span's padding box: the half-leading above the glyphs. */
        top: 0;
        width: 100%;
        font-size: var(--word-translation-font-size, 0.55em);
        text-align: center;
        line-height: 1;
        padding: 0.05em 0.1em;
        box-sizing: border-box;
        white-space: nowrap;
        opacity: 0.9;
        user-select: none;
        -webkit-user-select: none;
        pointer-events: none;
        z-index: 2;
        overflow: hidden;
    }

    .paragraphs-container :global(.translation-overlay.tapped) {
        animation: translation-shown 150ms ease-out;
    }

    @keyframes -global-translation-shown {
        from {
            opacity: 0;
        }
    }

    .far.leads {
        break-before: column;
        -webkit-column-break-before: always;
    }

    .far > i {
        display: block;
    }

    /* A height, or an engine drops the column it would leave empty. */
    .far > .column-end {
        height: 1px;
        break-after: column;
        -webkit-column-break-after: always;
    }

    /* One per column, absolute so they land on exact column boundaries
       and stay out of the multi-column flow. */
    .snap-target {
        position: absolute;
        top: 0;
        width: 1px;
        height: 1px;
        opacity: 0;
        pointer-events: none;
        scroll-snap-align: start;
    }

    @media (max-width: 768px) {
        .chapter-container {
            padding: 0;
        }

        .chapter {
            border: none;
            box-shadow: none;
        }
    }

    @media (max-width: 576px) {
        .chapter {
            padding: 6mm 0;
        }
    }
</style>
