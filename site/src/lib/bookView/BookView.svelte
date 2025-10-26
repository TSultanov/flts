<script lang="ts">
    import { goto } from "@mateothegreat/svelte5-router";
    import ChapterView from "./ChapterView.svelte";
    import WordView from "./WordView.svelte";
    import { route as r } from "@mateothegreat/svelte5-router";
    import type { UUID } from "../data/v2/db";
    import { getContext } from "svelte";
    import type { Library } from "../data/library";

    const { route } = $props();

    const bookId: UUID = route.result.path.params.bookId as UUID;
    const chapterId: number | null = $derived(
        route.result.path.params.chapterId
            ? (+route.result.path.params.chapterId as number)
            : null,
    );

    const library: Library = getContext("library");
    const chapters = $derived(library.getBookChapters(bookId));

    $effect(() => {
        if ($chapters && $chapters.length === 1) {
            goto(`/book/${bookId}/${$chapters[0].id}`);
        }
    });

    let sentenceWordIdToDisplay: [number, number, number] | null = $state(null);
</script>

{#if $chapters}
    <div class="container">
        {#if $chapters.length > 1}
            <div class="chapters">
                {#each $chapters as chapter}
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
                <ChapterView {bookId} {chapterId} bind:sentenceWordIdToDisplay />
            </div>
            <div class="word-view">
                {#if sentenceWordIdToDisplay}
                    <WordView {bookId} {sentenceWordIdToDisplay} />
                {:else}
                    Select word to show translation
                {/if}
            </div>
        {/if}
    </div>
{:else}
    <p>Failed to load book.</p>
{/if}

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
