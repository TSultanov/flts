<script lang="ts">
    import WordView from "./WordView.svelte";
    import type { UUID } from "../data/v2/db";
    import { getContext, type Snippet } from "svelte";
    import type { Library } from "../data/library";
    import { route, navigate } from "../../router";
    import ChapterView from "./ChapterView.svelte";
    import ChapterPlaceholderView from "./ChapterPlaceholderView.svelte";

    const params = $derived(route.params);

    const bookId = $derived(params.bookId! as UUID);
    const chapterId = $derived(
        params.chapterId != undefined ? parseInt(params.chapterId) : null,
    );

    const library: Library = getContext("library");
    const chapters = $derived(library.getBookChapters(bookId as UUID));

    $effect(() => {
        if ($chapters && $chapters.length === 1) {
            navigate("/book/:bookId/:chapterId", {
                params: {
                    bookId: bookId,
                    chapterId: $chapters[0].id.toString(),
                },
                search: {},
            });
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
                        <a href="/book/{bookId}/{chapter.id}"
                            >{chapter.title ? chapter.title : "<no title>"}</a
                        >
                    </p>
                {/each}
            </div>
        {/if}
        {#if chapterId != null}
            <div class="chapter-view">
                <ChapterView
                    {bookId}
                    {chapterId}
                    bind:sentenceWordIdToDisplay
                />
            </div>
            <div class="word-view">
                {#if sentenceWordIdToDisplay}
                    <WordView {bookId} {sentenceWordIdToDisplay} />
                {:else}
                    Select word to show translation
                {/if}
            </div>
        {:else}
            <div class="chapter-view">
                <ChapterPlaceholderView />
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
        overflow-x: none;
    }

    .word-view {
        flex: 0 1 300px;
        padding: 10px;
        border-left: 1px solid var(--background-color);
        overflow-y: auto;
    }
</style>
