<script lang="ts">
    import { goto } from "@mateothegreat/svelte5-router";
    import ChapterView from "./ChapterView.svelte";
    import WordView from "./WordView.svelte";
    import { route as r } from "@mateothegreat/svelte5-router";
    import type { BookChapterId, BookId, BookParagraphTranslationSentenceWordId, DatabaseSchema } from "../data/evolu/schema";
    import { getContext } from "svelte";
    import type { Books } from "../data/evolu/book";
    import { queryState } from "@evolu/svelte";
    import type { Evolu } from "@evolu/common";

    const { route } = $props();

    const bookId: BookId = route.result.path.params.bookId as BookId;
    const chapterId: BookChapterId | null = $derived(
        route.result.path.params.chapterId
            ? (route.result.path.params.chapterId as BookChapterId)
            : null,
    );

    const books: Books = getContext("books");
    const evolu: Evolu<DatabaseSchema> = getContext("evolu");

    const chaptersQuery = $derived(books.bookChapters(bookId));

    const chapters = queryState(evolu, () => chaptersQuery);

    $effect(() => {
        if (chapters.rows.length === 1) {
            goto(`/book/${bookId}/${chapters.rows[0].id}`);
        }
    });

    let sentenceWordIdToDisplay: BookParagraphTranslationSentenceWordId | null = $state(null);
</script>

<div class="container">
    {#if chapters.rows.length > 1}
        <div class="chapters">
            {#each chapters.rows as chapter}
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
            <ChapterView {chapterId} bind:sentenceWordIdToDisplay />
        </div>
        <div class="word-view">
            {#if sentenceWordIdToDisplay}
                <WordView {sentenceWordIdToDisplay} />
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
        overflow-y: auto;
    }

    .word-view {
        flex: 0 1 300px;
        padding: 10px;
        border-left: 1px solid var(--background-color);
        overflow-y: auto;
    }
</style>
