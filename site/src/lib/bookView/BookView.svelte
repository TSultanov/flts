<script lang="ts">
    import { getContext, onMount } from "svelte";
    import type { Library, LibraryBook } from "../library.svelte";
    import { goto } from "@mateothegreat/svelte5-router";
    import ChapterView from "./ChapterView.svelte";
    import WordView from "./WordView.svelte";

    const { route } = $props();

    const bookId: number = parseInt(route.result.path.params.bookId);
    const chapterId: number | null = route.result.path.params.chapterId
        ? parseInt(route.result.path.params.chapterId)
        : null;

    const library: Library = getContext("library");
    let book: LibraryBook | null = $state(null);
    onMount(async () => {
        book = await library.getBook(bookId);
        if (book?.chapters.length === 1) {
            goto(`/book/${bookId}/${book.chapters[0].id}`);
        }
    });

    let sentenceWordIdToDisplay: number | null = $state(null);
</script>

<div class="container">
    <!-- {#if book?.chapters && book.chapters.length > 1} -->
    {#if book?.chapters}
        <div class="chapters">
            {#each book.chapters as chapter}
                <p>
                    <a href="/book/{bookId}/{chapter.id}"
                        >{chapter.title ? chapter.title : "<no title>"}</a
                    >
                </p>
            {/each}
        </div>
    {/if}
    {#if chapterId}
        <div class="chapter-view">
            <ChapterView {chapterId} bind:sentenceWordId={sentenceWordIdToDisplay} />
        </div>
    {/if}
    <div class="word-view">
        {#if sentenceWordIdToDisplay}
            <WordView wordId={sentenceWordIdToDisplay} />
        {:else}
            Select word to show translation
        {/if}
    </div>
</div>

<style>
    .container {
        display: flex;
        flex-direction: row;
        height: 100%;
    }

    .chapter-view {
        flex: 1 1 auto;
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
