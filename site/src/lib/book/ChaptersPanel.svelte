<script lang="ts">
    import Fa from "svelte-fa";
    import { faChevronLeft, faChevronRight } from "@fortawesome/free-solid-svg-icons";
    import { getContext } from "svelte";
    import type { ChapterMetaView } from "../data/library";
    import type { UUID } from "../data/uuid";
    import ResizableOverlayPanel from "../widgets/ResizableOverlayPanel.svelte";
    import CircularProgress from "../widgets/CircularProgress.svelte";
    import {
        SUMMARY_STATUS_KEY,
        type BookSummaryStatusStore,
    } from "./BookSummaryStatusStore.svelte";

    const {
        bookId,
        chapters,
        currentChapterId,
    }: {
        bookId: UUID;
        chapters: ChapterMetaView[];
        currentChapterId: number | null;
    } = $props();

    const summaryStatusHolder: { store: BookSummaryStatusStore | null } =
        getContext(SUMMARY_STATUS_KEY);

    let isOpen = $state(false);
    let width = $state(260);

    function handleChapterClick() {
        if (!isOpen) return;
        isOpen = false;
    }
</script>

<ResizableOverlayPanel
    side="left"
    bind:expanded={isOpen}
    bind:size={width}
    minSize={150}
    maxSizeRatio={0.5}
    shortcut="c"
    testId="chapters-panel"
>
    <nav class="chapters">
        {#each chapters as chapter}
            <p
                data-testid="chapter-row"
                data-chapter-id={chapter.id}
                class:current={chapter.id === currentChapterId}
                class:dim={summaryStatusHolder.store
                    ? !summaryStatusHolder.store.isGenerated(chapter.id)
                    : false}
            >
                <a
                    href="/book/{bookId}/{chapter.id}"
                    onclick={handleChapterClick}
                >
                    {chapter.title ? chapter.title : "<no title>"}
                </a>
                {#if chapter.translationRatio < 1}
                    <span class="ratio" data-testid="chapter-translation-ratio">
                        {(chapter.translationRatio * 100).toFixed(0)}%
                    </span>
                {/if}
                {#if summaryStatusHolder.store?.isActivelyGenerating(chapter.id)}
                    <span class="spinner" data-testid="summary-spinner">
                        <CircularProgress
                            size="0.9em"
                            strokeWidth={3}
                            color="var(--text-color)"
                            indeterminate
                        />
                    </span>
                {/if}
            </p>
        {/each}
    </nav>
</ResizableOverlayPanel>

<button
    type="button"
    class="edge-handle"
    class:open={isOpen}
    style="--panel-width: {width}px"
    aria-label={isOpen ? "Hide chapters" : "Show chapters"}
    aria-expanded={isOpen}
    data-testid="chapters-panel-handle"
    onclick={() => (isOpen = !isOpen)}
>
    <Fa icon={isOpen ? faChevronLeft : faChevronRight} />
</button>

<style>
    .chapters {
        flex: 1 1 auto;
        padding: 10px;
        overflow-y: auto;
        overflow-x: hidden;
    }

    .chapters p {
        margin: 0.25em 0;
        display: flex;
        align-items: center;
        gap: 0.4em;
    }

    .chapters p a {
        flex: 1 1 auto;
        min-width: 0;
    }

    .chapters .current {
        outline: 1px dotted var(--selected-color);
    }

    .chapters p.dim {
        opacity: 0.5;
    }

    .chapters .ratio {
        flex: 0 0 auto;
        color: var(--text-color-muted);
        font-size: 0.85em;
        font-variant-numeric: tabular-nums;
    }

    .chapters .spinner {
        flex: 0 0 auto;
        display: inline-block;
        vertical-align: middle;
    }

    .edge-handle {
        position: absolute;
        top: 50%;
        left: 0;
        z-index: 11;
        /* Centering via negative margin so the global `button:active`
           transform from app.css doesn't override our centering and yank
           the button out of position mid-click. */
        margin-top: -28px;
        width: 22px;
        height: 56px;
        padding: 0;
        border: 1px solid var(--background-color);
        border-left: none;
        border-radius: 0 6px 6px 0;
        background-color: var(--dialog-background);
        color: var(--dialog-text);
        cursor: pointer;
        display: flex;
        align-items: center;
        justify-content: center;
        box-shadow: 2px 0 4px rgba(0, 0, 0, 0.15);
        transition: left 180ms ease;
    }

    .edge-handle:hover:not(:disabled) {
        background-color: var(--button-cancel-hover);
    }

    .edge-handle :global(svg) {
        pointer-events: none;
    }

    .edge-handle.open {
        left: var(--panel-width);
    }

    @media (max-width: 576px) {
        .edge-handle {
            width: 13px;
            height: 40px;
        }
    }
</style>
