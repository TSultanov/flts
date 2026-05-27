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

    // One summary-status store per opened book. Held in a reactive
    // holder so the context value can be set once at init while the
    // underlying store is swapped when bookId changes. The store is
    // null between mount and the first $effect tick; consumers default
    // to "fully ready" during that sub-frame window.
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
                        translationRatio={chapters.current?.find((c) => c.id === chapterId)?.translationRatio ?? 0}
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
            <WordView {bookId} {selection} />
        {/if}
    </div>
{:else}
    <p>Failed to load book.</p>
{/if}

<style>
    /* Vertical flex: chapter-area fills the remaining space, WordView's
       slot takes its collapsed-size height at the bottom. The expanded
       WordView body overflows up via absolute positioning inside its own
       slot, so opening the word view never resizes .chapter-area. */
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
