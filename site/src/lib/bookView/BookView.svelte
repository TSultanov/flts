<script lang="ts">
    import { goto } from "@mateothegreat/svelte5-router";
    import ChapterView from "./ChapterView.svelte";
    import WordView from "./WordView.svelte";
    import { route as r } from "@mateothegreat/svelte5-router";
    import type { UUID } from "../data/v2/db";
    import { books, type ChapterId, type TranslatedWordId } from "../data/v2/book.svelte";
    import { onMount } from "svelte";

    const { route } = $props();

    const bookUid: UUID = route.result.path.params.bookId as UUID;
    const chapterId: ChapterId | null = $derived(
        route.result.path.params.chapterId
            ? ({
                  chapter: parseInt(route.result.path.params.chapterId),
              } as ChapterId)
            : null,
    );

    $inspect(route);
    $inspect(chapterId);

    const book = $derived(books.getBook(bookUid));

    onMount(() => {
        book.then((book) => {
            if (book && book.chapters.length === 1) {
                // chapterUid = $book.chapters[0].uid;
                goto(`/book/${bookUid}/${book.chapters[0].id.chapter}`);
            }
        });
    });

    let sentenceWordIdToDisplay: TranslatedWordId | null = $state(null);
</script>

{#await book}
    <p>Loading...</p>
{:then book}
    {#if book}
        <div class="container">
            {#if book?.chapters}
                <div class="chapters">
                    {#each book.chapters as chapter}
                        <p>
                            <a use:r href="/book/{bookUid}/{chapter.id.chapter}"
                                >{chapter.title
                                    ? chapter.title
                                    : "<no title>"}</a
                            >
                        </p>
                    {/each}
                </div>
            {/if}
            {#if chapterId}
                <div class="chapter-view">
                    <ChapterView
                        {book}
                        {chapterId}
                        bind:sentenceWordIdToDisplay={sentenceWordIdToDisplay}
                    />
                </div>
                <div class="word-view">
                    {#if sentenceWordIdToDisplay}
                        <WordView {book} {sentenceWordIdToDisplay} />
                    {:else}
                        Select word to show translation
                    {/if}
                </div>
            {/if}
        </div>
    {:else}
        <p>Failed to load book.</p>
    {/if}
{/await}

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
