<script lang="ts">
    import { getContext, onMount } from "svelte";
    import { type Library } from "../library.svelte";
    import ParagraphView from "./ParagraphView.svelte";
    import type { ChapterId, IBook, ParagraphId, TranslatedWordId } from "../data/v2/book.svelte";

    let {
        book,
        chapterId,
        sentenceWordIdToDisplay = $bindable(),
    }: {
        book: IBook,
        chapterId: ChapterId;
        sentenceWordIdToDisplay: TranslatedWordId | null;
    } = $props();

    const chapter = $derived(book.getChapterView(chapterId));

    function chapterClick(e: MouseEvent) {
        const target = document.elementFromPoint(e.clientX, e.clientY) as HTMLElement;
        if (target && target.classList.contains("word-span")) {
            sentenceWordIdToDisplay = {
                chapter: parseInt(target.dataset["chapter"]!),
                paragraph: parseInt(target.dataset["paragraph"]!),
                sentence: parseInt(target.dataset["sentence"]!),
                word: parseInt(target.dataset["word"]!),
            } as TranslatedWordId;
        } else {
            sentenceWordIdToDisplay = null;
        }
    }

    let sectionContentWidth = $state(200);
</script>

<div class="chapter-container">
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <section class="chapter" onclick={chapterClick}>
        <div
            class="paragraphs-container"
            style="column-width: {sectionContentWidth}px"
            bind:clientHeight={sectionContentWidth}
        >
            {#if chapter}
                {#each chapter.paragraphs as paragraph}
                    <ParagraphView
                        {paragraph}
                        {sentenceWordIdToDisplay}
                    />
                {/each}
            {/if}
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
        padding: calc(1cm - 2px) calc(1.5cm - 2px);
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
        padding: 2px;
        overflow-x: auto;
        scroll-snap-type: x mandatory;
        column-gap: 2cm;
    }
    
    :global(.paragraphs-container > *) {
        scroll-snap-align: center;
        scroll-snap-stop: always;
    }
</style>
