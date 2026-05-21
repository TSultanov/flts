<script lang="ts">
    import WordView from "./WordView.svelte";
    import type { UUID } from "../data/uuid";
    import { getContext, type Snippet } from "svelte";
    import { SvelteMap } from "svelte/reactivity";
    import type { BookReadingState, Library } from "../data/library";
    import { route, navigate } from "../../router";
    import ChapterView from "./ChapterView.svelte";
    import ChapterPlaceholderView from "./ChapterPlaceholderView.svelte";
    import ChaptersPanel from "./ChaptersPanel.svelte";
    import type { WordSelection } from "./ParagraphViewModel.svelte";

    const params = $derived(route.params);

    const bookId = $derived(params.bookId! as UUID);
    const chapterId = $derived(
        params.chapterId != undefined ? parseInt(params.chapterId) : null,
    );

    const library: Library = getContext("library");
    const chapters = $derived(library.getBookChapters(bookId as UUID));

    let readingState: BookReadingState | null = $state(null);
    // Per-chapter session positions. Seeded from the backend reading-state
    // on book open, then kept in sync via the ChapterView onPositionChange
    // callback. Survives intra-session chapter navigation; the backend
    // store still gets every save for cross-session persistence.
    let positionByChapter = $state(
        new SvelteMap<number, { paragraphId: number; pageOffset: number }>(),
    );
    let readingStateRequestId = 0;
    let initialNavigationDone = $state(false);
    let previousBookId: UUID | null = null;

    $effect(() => {
        if (previousBookId !== bookId) {
            previousBookId = bookId;
            initialNavigationDone = false;
            readingState = null;
            positionByChapter.clear();
            const currentRequest = ++readingStateRequestId;
            library
                .getBookReadingState(bookId as UUID)
                .then((state) => {
                    if (currentRequest === readingStateRequestId) {
                        readingState = state;
                        if (state) {
                            positionByChapter.set(state.chapterId, {
                                paragraphId: state.paragraphId,
                                pageOffset: state.pageOffset,
                            });
                        }
                    }
                })
                .catch((err) => console.error("Failed to load reading state", err));
        }
    });

    function handlePositionChange(paragraphId: number, pageOffset: number) {
        if (chapterId == null) return;
        positionByChapter.set(chapterId, { paragraphId, pageOffset });
        library
            .saveBookReadingState(
                bookId as UUID,
                chapterId,
                paragraphId,
                pageOffset,
            )
            .catch((err) => console.error("Failed to save reading state", err));
    }

    $effect(() => {
        const list = chapters.current;
        if (!list || initialNavigationDone) {
            return;
        }

        if (chapterId != null) {
            initialNavigationDone = true;
            return;
        }

        const state = readingState;
        const chapterFromState = state
            ? list.find((ch) => ch.id === state.chapterId)
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

        if (list.length === 1) {
            initialNavigationDone = true;
            navigate("/book/:bookId/:chapterId", {
                params: {
                    bookId: bookId,
                    chapterId: list[0].id.toString(),
                },
                search: {},
            });
        }
    });

    let selection: WordSelection | null = $state(null);
</script>

{#if chapters.current}
    <div class="container">
        <div class="chapter-view">
            {#if chapters.current.length > 1}
                <ChaptersPanel
                    {bookId}
                    chapters={chapters.current}
                    currentChapterId={chapterId}
                />
            {/if}
            {#if chapterId != null}
                {#key chapterId}
                    <ChapterView
                        {bookId}
                        {chapterId}
                        initialParagraphId={positionByChapter.get(chapterId)?.paragraphId ?? null}
                        initialPageOffset={positionByChapter.get(chapterId)?.pageOffset ?? 0}
                        onPositionChange={handlePositionChange}
                        bind:selection
                    />
                {/key}
            {:else}
                <ChapterPlaceholderView />
            {/if}
        </div>
        {#if chapterId != null}
            <div class="word-view">
                {#if selection}
                    <WordView {bookId} {selection} />
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
        display: grid;
        grid-template-columns: auto 300px;
        height: 100%;
    }

    @media (max-aspect-ratio: 1/1) {
        .container {
            grid-template-columns: auto;
            grid-template-rows: auto 300px;
        }
    }

    .chapter-view {
        position: relative;
        flex: 1 1 auto;
        hyphens: auto;
        overflow: hidden;
    }

    .word-view {
        padding: 10px;
        border-left: 1px solid var(--background-color);
        overflow-y: auto;
    }
</style>
