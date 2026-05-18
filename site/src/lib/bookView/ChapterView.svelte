<script lang="ts">
    import { getContext, onDestroy, onMount, setContext } from "svelte";
    import type { UUID } from "../data/v2/db";
    import ParagraphView from "./ParagraphView.svelte";
    import type { Library } from "../data/library";
    import type { WordSelection } from "./ParagraphViewModel.svelte";
    import { ChapterViewModel, type WordClickInfo } from "./ChapterViewModel.svelte";
    import { CHAPTER_STORE_KEY } from "./ChapterParagraphsStore.svelte";

    let {
        selection = $bindable(),
        bookId,
        chapterId,
        initialParagraphId = null,
        initialPageOffset = 0,
    }: {
        selection: WordSelection | null;
        bookId: UUID;
        chapterId: number;
        initialParagraphId?: number | null;
        initialPageOffset?: number;
    } = $props();

    const library: Library = getContext("library");

    let paragraphsContainer = $state<HTMLDivElement | null>(null);
    let sectionContentWidth = $state(200);

    const vm = new ChapterViewModel(library, {
        get bookId() { return bookId; },
        get chapterId() { return chapterId; },
        get initialParagraphId() { return initialParagraphId; },
        get initialPageOffset() { return initialPageOffset; },
        get container() { return paragraphsContainer; },
    });

    setContext(CHAPTER_STORE_KEY, vm.store);

    function handleWordClick(info: WordClickInfo) {
        selection = vm.handleWordClick(info);
    }

    function handleBackgroundClick(e: MouseEvent) {
        const target = e.target instanceof Element ? e.target : null;
        if (target?.closest(".word-span")) return;
        selection = null;
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
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <section class="chapter" onclick={handleBackgroundClick}>
        <div
            class="paragraphs-container"
            class:is-ready={vm.isInitiallyReady}
            style="column-width: {sectionContentWidth}px"
            bind:clientHeight={sectionContentWidth}
            bind:this={paragraphsContainer}
            onscroll={() => vm.handleScroll()}
        >
            {#each vm.paragraphIds as paragraphId (paragraphId)}
                <ParagraphView
                    {bookId}
                    {paragraphId}
                    {selection}
                    mounted={vm.isMounted(paragraphId)}
                    onWordClick={handleWordClick}
                    onReady={() => vm.registerParagraphReady(paragraphId)}
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
        opacity: 0;
    }

    .paragraphs-container.is-ready {
        opacity: 1;
    }

    :global(.paragraphs-container > *) {
        scroll-snap-align: center;
        scroll-snap-stop: always;
    }

    /* Touch devices use break-inside: auto, so a single paragraph can
       flow across multiple columns. Scroll-snap puts one snap point per
       wrapper — incompatible with a wrapper that spans pages, since
       restoring to "page N of paragraph X" gets snapped back to the
       wrapper's center. Disable snap on coarse-pointer devices; the
       column-fill: auto layout still yields page-shaped reading without
       it. */
    @media (pointer: coarse) {
        .paragraphs-container {
            scroll-snap-type: none;
        }
        :global(.paragraphs-container > *) {
            scroll-snap-align: none;
            scroll-snap-stop: normal;
        }
    }
</style>
