<script lang="ts">
    import WordView from "./WordView.svelte";
    import type { UUID } from "../data/uuid";
    import { getContext, onDestroy, setContext, type Snippet } from "svelte";
    import { SvelteMap } from "svelte/reactivity";
    import type { BookReadingState, Library } from "../data/library";
    import { route, navigate } from "../../router";
    import ChapterView from "./ChapterView.svelte";
    import ChapterPlaceholderView from "./ChapterPlaceholderView.svelte";
    import ChaptersPanel from "./ChaptersPanel.svelte";
    import type { WordSelection } from "./ParagraphViewModel.svelte";
    import {
        BookSummaryStatusStore,
        SUMMARY_STATUS_KEY,
    } from "./BookSummaryStatusStore.svelte";

    const params = $derived(route.params);

    const bookId = $derived(params.bookId! as UUID);
    const chapterId = $derived(
        params.chapterId != undefined ? parseInt(params.chapterId) : null,
    );

    const library: Library = getContext("library");
    const chapters = $derived(library.getBookChapters(bookId as UUID));

    // One store per book, in a holder so the context value is set once at
    // init while the store swaps on bookId. Null until the first $effect
    // tick; consumers default to "fully ready" for that sub-frame.
    const summaryStatusHolder: { store: BookSummaryStatusStore | null } =
        $state({ store: null });
    setContext(SUMMARY_STATUS_KEY, summaryStatusHolder);
    let summaryStatusForBookId: UUID | null = null;
    $effect(() => {
        if (summaryStatusForBookId !== bookId) {
            summaryStatusHolder.store?.dispose();
            summaryStatusHolder.store = new BookSummaryStatusStore(bookId);
            summaryStatusForBookId = bookId;
        }
    });
    onDestroy(() => summaryStatusHolder.store?.dispose());

    let readingState: BookReadingState | null = $state(null);
    // Per-chapter session positions, so intra-session chapter navigation
    // keeps its place. The backend still gets every save.
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
                .catch((err) =>
                    console.error("Failed to load reading state", err),
                );
        }
    });

    function handlePositionChange(
        chapterId: number,
        paragraphId: number,
        pageOffset: number,
    ) {
        // chapterId comes from the emitting VM, never the ambient derived:
        // on an A→B switch the derived is already B when A's teardown
        // flushes, which would file A's paragraph under chapter B.
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
    <div class="chapter-view">
        <div class="chapter-area">
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
                        translationRatio={chapters.current?.find(
                            (c) => c.id === chapterId,
                        )?.translationRatio ?? 0}
                        initialParagraphId={positionByChapter.get(chapterId)
                            ?.paragraphId ?? null}
                        initialPageOffset={positionByChapter.get(chapterId)
                            ?.pageOffset ?? 0}
                        onPositionChange={handlePositionChange}
                        bind:selection
                    />
                {/key}
            {:else}
                <ChapterPlaceholderView />
            {/if}
        </div>
        {#if chapterId != null}
            <WordView {bookId} {selection} />
        {/if}
    </div>
{:else}
    <p>Failed to load book.</p>
{/if}

<style>
    /* WordView's slot only ever takes its collapsed height; the expanded
       body overflows upward absolutely, so opening the word view never
       resizes .chapter-area (which would reflow the page columns). */
    .chapter-view {
        display: flex;
        flex-direction: column;
        position: relative;
        height: 100%;
        hyphens: auto;
        overflow: hidden;
    }

    .chapter-area {
        flex: 1 1 auto;
        min-height: 0;
        position: relative;
        overflow: hidden;
    }
</style>
