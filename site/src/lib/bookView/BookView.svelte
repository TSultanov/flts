<script lang="ts">
    import { getContext } from "svelte";
    import { type Library } from "../library.svelte";
    import { goto } from "@mateothegreat/svelte5-router";
    import ChapterView from "./ChapterView.svelte";
    import WordView from "./WordView.svelte";
    import { route as r } from "@mateothegreat/svelte5-router";

    const { route } = $props();
    $inspect(route);

    const bookId: number = parseInt(route.result.path.params.bookId);
    const chapterId: number | null = $derived(
        route.result.path.params.chapterId
            ? parseInt(route.result.path.params.chapterId)
            : null,
    );

    const library: Library = getContext("library");
    const book = $derived(library.getBook(bookId));

    $effect(() => {
        if ($book?.chapters.length === 1) {
            // chapterId = $book.chapters[0].id;
            goto(`/book/${bookId}/${$book.chapters[0].id}`);
        }
    });

    let sentenceWordIdToDisplay: number | null = $state(null);
</script>

<div class="container">
    <!-- {#if book?.chapters && book.chapters.length > 1} -->
    {#if $book?.chapters}
        <div class="chapters">
            {#each $book.chapters as chapter}
                <p>
                    <a use:r href="/book/{bookId}/{chapter.id}"
                        >{chapter.title ? chapter.title : "<no title>"}</a
                    >
                </p>
            {/each}
        </div>
    {/if}
    {#if chapterId}
        <div class="chapter-view">
            <ChapterView
                {chapterId}
                bind:sentenceWordId={sentenceWordIdToDisplay}
            />
        </div>
        <div class="word-view">
            {#if sentenceWordIdToDisplay}
                <WordView wordId={sentenceWordIdToDisplay} />
            {:else}
                Select word to show translation
            {/if}
        </div>
    {/if}
</div>

<style>
    .container {
        display: flex;
        flex-direction: row;
        height: 100%;
    }

    .chapter-view {
        flex: 1 1 auto;
        hyphens: auto;
    }

    .chapters {
        flex: 0 1 150px;
        padding: 10px;
        border-right: 1px solid var(--background-color);
    }

    .word-view {
        flex: 0 1 300px;
        padding: 10px;
        border-left: 1px solid var(--background-color);
        overflow-y: auto;
    }
</style>
