<script lang="ts">
    import { getContext, onMount } from "svelte";
    import type { Library, LibraryBookChapter } from "../library.svelte";
    import ParagraphView from "./ParagraphView.svelte";

    let { chapterId, sentenceWordId = $bindable() }: {
        chapterId: number,
        sentenceWordId: number | null,
     } = $props();

    let chapter: LibraryBookChapter | null = $state(null);

    const library: Library = getContext("library");

    onMount(async () => {
        chapter = await library.getChapter(chapterId);
    });

    function chapterClick(e: MouseEvent) {
        const target = document.elementFromPoint(e.clientX, e.clientY);
        if (target && target.classList.contains("word-span")) {
            const wordId = parseInt(target.id.replace("sentence-word-", ""));
            sentenceWordId = wordId;
        } else {
            sentenceWordId = null;
        }
    }
</script>

<div class="chapter-container">
<!-- svelte-ignore a11y_click_events_have_key_events -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<section class="chapter" onclick="{chapterClick}">
{#if chapter}
{#each chapter.paragraphs as paragraph}
    <ParagraphView paragraphId={paragraph.id} sentenceWordId={sentenceWordId} />
{/each}
{/if}
</section>
</div>

<style>
    .chapter-container {
        background-color: var(--hover-color);
        padding: 10px 25px;
        justify-content: center;
        height: 100%;
        overflow-y: auto;
    }

    .chapter {
        padding: 1cm 1.5cm;
        max-width: 800px;
        margin: 0 auto;
        border: 1px solid var(--background-color);
        background-color: white;
        box-shadow: 2px 2px var(--background-color);
        text-align: justify;
        line-height: 2;
        min-height: 100%;
    }
</style>
