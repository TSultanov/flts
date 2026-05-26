<script lang="ts">
    import { getContext, onDestroy, onMount, setContext } from "svelte";
    import type { UUID } from "../data/uuid";
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
        onPositionChange,
    }: {
        selection: WordSelection | null;
        bookId: UUID;
        chapterId: number;
        initialParagraphId?: number | null;
        initialPageOffset?: number;
        onPositionChange?: (paragraphId: number, pageOffset: number) => void;
    } = $props();

    const library: Library = getContext("library");

    let paragraphsContainer = $state<HTMLDivElement | null>(null);
    // Bound to clientHeight and fed into the `column-width` CSS property
    // as a minimum-width hint. Any value ≥ clientWidth forces the browser
    // to render exactly one visible column; using clientHeight guarantees
    // that for any normal aspect ratio.
    let columnWidthHint = $state(200);
    // Snap targets must sit at multiples of the *visible* column width
    // (`clientWidth`), which is what one "page" actually is.
    let containerVisibleWidth = $state(800);

    const vm = new ChapterViewModel(library, {
        get bookId() { return bookId; },
        get chapterId() { return chapterId; },
        get initialParagraphId() { return initialParagraphId; },
        get initialPageOffset() { return initialPageOffset; },
        get container() { return paragraphsContainer; },
        get onPositionChange() { return onPositionChange; },
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
            {#each vm.paragraphIds as paragraphId (paragraphId)}
                <ParagraphView
                    {bookId}
                    {chapterId}
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
        position: relative;
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

    /* One snap target per column. Absolutely positioned so they sit at
       exact column boundaries regardless of paragraph layout, and so
       they don't participate in the multi-column flow. */
    .snap-target {
        position: absolute;
        top: 0;
        width: 1px;
        height: 1px;
        opacity: 0;
        pointer-events: none;
        scroll-snap-align: start;
    }
</style>
