<script lang="ts">
    import { getContext, onMount } from "svelte";
    import type { Library, LibraryBookChapter } from "../library.svelte";
    import ParagraphView from "./ParagraphView.svelte";

    const { chapterId }: { chapterId: number } = $props();

    let chapter: LibraryBookChapter | null = $state(null);

    const library: Library = getContext("library");

    onMount(async () => {
        chapter = await library.getChapter(chapterId);
    });
</script>

<div class="chapter-container">
<section class="chapter">
{#if chapter}
{#each chapter.paragraphs as paragraph}
    <ParagraphView paragraphId={paragraph.id} />
{/each}
{/if}
</section>
</div>

<style>
    .chapter-container {
        background-color: var(--hover-color);
        padding: 10px;
        display: flex;
        justify-content: center;
        height: 100%;
        overflow-y: auto;
    }

    .chapter {
        padding: 1cm 1.5cm;
        max-width: 900px;
        border: 1px solid var(--background-color);
        background-color: white;
        box-shadow: 2px 2px var(--background-color);
        text-align: justify;
        line-height: 2;
    }
</style>
