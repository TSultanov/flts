<script lang="ts">
    import WordView from "./WordView.svelte";
    import type { UUID } from "../data/v2/db";
    import { getContext, type Snippet } from "svelte";
    import type { BookReadingState, Library } from "../data/library";
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

    let readingState: BookReadingState | null = $state(null);
    let readingStateRequestId = 0;
    let initialNavigationDone = $state(false);
    let previousBookId: UUID | null = null;

    $effect(() => {
        if (previousBookId !== bookId) {
            previousBookId = bookId;
            initialNavigationDone = false;
            readingState = null;
            const currentRequest = ++readingStateRequestId;
            library
                .getBookReadingState(bookId as UUID)
                .then((state) => {
                    if (currentRequest === readingStateRequestId) {
                        readingState = state;
                    }
                })
                .catch((err) => console.error("Failed to load reading state", err));
        }
    });

    $effect(() => {
        if (!$chapters || initialNavigationDone) {
            return;
        }

        if (chapterId != null) {
            initialNavigationDone = true;
            return;
        }

        const state = readingState;
        const chapterFromState = state
            ? $chapters.find((ch) => ch.id === state.chapterId)
            : null;

        if (chapterFromState) {
            initialNavigationDone = true;
            navigate("/book/:bookId/:chapterId", {
                params: {
                    bookId: bookId,
                    chapterId: chapterFromState.id.toString(),
                },
                search: {},
            });
            return;
        }

        if ($chapters.length === 1) {
            initialNavigationDone = true;
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
    <div class="container {$chapters.length <= 1 ? "container-twocolumn" : ""}">
        {#if $chapters.length > 1}
            <div class="chapters">
                {#each $chapters as chapter}
                    <p class="{chapter.id === chapterId ? "current" : ""}">
                        <a href="/book/{bookId}/{chapter.id}"
                            >{chapter.title ? chapter.title : "<no title>"}</a
                        >
                    </p>
                {/each}
            </div>
        {/if}
        {#if chapterId != null}
            <div class="chapter-view">
                {#key chapterId}
                    <ChapterView
                        {bookId}
                        {chapterId}
                        initialParagraphId={readingState && readingState.chapterId === chapterId
                            ? readingState.paragraphId
                            : null}
                        bind:sentenceWordIdToDisplay
                    />
                {/key}
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
        display: grid;
        /* flex-direction: row; */
        grid-template-columns: 150px auto 300px;
        height: 100%;
    }

    .container-twocolumn {
        grid-template-columns: auto 300px;
    }

    @media (max-aspect-ratio: 1/1) {
        .container {
            grid-template-columns: 150px auto;
            grid-template-rows: auto 300px;
        }

        .container-twocolumn {
            grid-template-columns: auto;
        }

        .word-view {
            grid-row: 2 / 3;
            grid-column: 2 / 3;
        }

        .container-twocolumn .word-view {
            grid-row: 2 / 3;
            grid-column: 1 / 2;
        }

        .chapters {
            grid-row: 1 / 3;
        }
    }

    .chapter-view {
        flex: 1 1 auto;
        hyphens: auto;
        overflow: auto;
    }

    .chapters {
        padding: 10px;
        border-right: 1px solid var(--background-color);
        overflow-y: auto;
        overflow-x: none;
    }

    .chapters .current {
        outline: 1px dotted var(--selected-color);
    }

    .word-view {
        padding: 10px;
        border-left: 1px solid var(--background-color);
        overflow-y: auto;
    }
</style>
