<script lang="ts">
    import { getContext } from "svelte";
    import type { BookChapterId, BookParagraphTranslationSentenceWordId, DatabaseSchema } from "../data/evolu/schema";
    import ParagraphView from "./ParagraphView.svelte";
    import type { Books } from "../data/evolu/book";
    import type { Evolu } from "@evolu/common";
    import { queryState } from "@evolu/svelte";

    let {
        chapterId,
        sentenceWordIdToDisplay = $bindable(),
    }: {
        chapterId: BookChapterId;
        sentenceWordIdToDisplay: BookParagraphTranslationSentenceWordId | null;
    } = $props();

    const books: Books = getContext("books");
    const evolu: Evolu<DatabaseSchema> = getContext("evolu");

    const paragraphsQuery = $derived(books.paragraphs(chapterId));

    const paragraphs = queryState(evolu, () => paragraphsQuery);

    function chapterClick(e: MouseEvent) {
        const target = document.elementFromPoint(e.clientX, e.clientY) as HTMLElement;
        if (target && target.classList.contains("word-span")) {
            const data = target.dataset["word"] as BookParagraphTranslationSentenceWordId;
            sentenceWordIdToDisplay = data;
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
            bind:clientWidth={sectionContentWidth}
        >
            {#each paragraphs.rows as paragraph}
                <ParagraphView {paragraph} {sentenceWordIdToDisplay} />
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
