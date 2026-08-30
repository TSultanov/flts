<script lang="ts">
    import Fa from "svelte-fa";
    import { faLanguage } from "@fortawesome/free-solid-svg-icons";
    import { getContext, type Snippet } from "svelte";
    import CircularProgress from "../widgets/CircularProgress.svelte";
    import type { Library, Mark } from "../data/library";
    import type { UUID } from "../data/uuid";
    import {
        ParagraphViewModel,
        type WordSelection,
    } from "./ParagraphViewModel.svelte";
    import WordSpan from "./WordSpan.svelte";
    import {
        CHAPTER_STORE_KEY,
        type ChapterParagraphsStore,
    } from "./ChapterParagraphsStore.svelte";
    import {
        SUMMARY_STATUS_KEY,
        type BookSummaryStatusStore,
    } from "./BookSummaryStatusStore.svelte";

    let {
        bookId,
        chapterId,
        paragraphId,
        selection = null,
        mounted = true,
        onWordClick,
        isWordRevealed,
        onReady,
    }: {
        bookId: UUID;
        chapterId: number;
        paragraphId: number;
        selection?: WordSelection | null;
        mounted?: boolean;
        onWordClick: (info: {
            paragraphId: number;
            sentence: number;
            word: number;
            flatIndex: number;
        }) => void;
        isWordRevealed: (flatIndex: number) => boolean;
        onReady?: () => void;
    } = $props();

    const library: Library = getContext("library");
    const store: ChapterParagraphsStore = getContext(CHAPTER_STORE_KEY);
    const summaryStatusHolder: { store: BookSummaryStatusStore | null } =
        getContext(SUMMARY_STATUS_KEY);
    // "ready" for the sub-frame before BookView's $effect builds the store.
    const canTranslate = $derived(
        summaryStatusHolder.store?.canTranslate(chapterId) ?? true,
    );
    const vm = new ParagraphViewModel(library, store, {
        get bookId() {
            return bookId;
        },
        get paragraphId() {
            return paragraphId;
        },
        get selection() {
            return selection;
        },
    });

    let firedReady = false;
    $effect(() => {
        if (firedReady) return;
        if (vm.isReady) {
            firedReady = true;
            onReady?.();
        }
    });
</script>

<!-- Wraps a segment in its emphasis. Marks arrive canonically ordered and are
     empty for almost every segment, so the nesting stays flat. -->
{#snippet marked(
    marks: Mark[] | undefined,
    kids: Snippet,
)}{#if marks?.includes("strong")}<strong
            >{#if marks.includes("emphasis")}<em>{@render kids()}</em
                >{:else}{@render kids()}{/if}</strong
        >{:else if marks?.includes("emphasis")}<em>{@render kids()}</em
        >{:else}{@render kids()}{/if}{/snippet}

<div class="paragraph-wrapper" data-paragraph-id={paragraphId}>
    {#if mounted && !vm.segments}
        <button
            class="translate"
            aria-label="Translate paragraph"
            title={canTranslate
                ? "Translate paragraph"
                : "Waiting for chapter summaries…"}
            onclick={(e) => vm.translate(!(e.metaKey || e.ctrlKey))}
            disabled={vm.isTranslating || !vm.originalText || !canTranslate}
        >
            {#if vm.isTranslating}
                <CircularProgress
                    value={vm.progressChars}
                    max={vm.expectedChars}
                    size="1.2em"
                    strokeWidth={4}
                />
            {:else}
                <Fa icon={faLanguage} />
            {/if}
        </button>
    {:else}
        <div></div>
    {/if}
    {#if vm.segments}
        <!-- Both branches render the same segments under the same wrappers,
             so a paragraph's height cannot change when it virtualizes. Write
             the tags without gaps: whitespace here becomes text. -->
        <p>
            {#if !mounted}{#each vm.runs ?? [] as run, i (i)}{#snippet runBody()}{run.kind ===
                        "text"
                            ? run.text
                            : ""}{/snippet}{#if run.kind === "break"}<br
                        />{:else}{@render marked(
                            run.marks,
                            runBody,
                        )}{/if}{/each}{:else}
                {#each vm.segments as seg, i (i)}
                    {#snippet body()}{#if seg.kind === "gap"}{seg.text}{:else if seg.kind === "word"}<WordSpan
                                text={seg.text}
                                sentence={seg.sentence}
                                word={seg.word}
                                flatIndex={seg.flatIndex}
                                translation={seg.translation}
                                manualShown={isWordRevealed(seg.flatIndex)}
                                familiarity={seg.familiarity}
                                selected={vm.isSelected(seg.sentence, seg.word)}
                                onClick={(w) =>
                                    onWordClick({ paragraphId, ...w })}
                            />{/if}{/snippet}{#if seg.kind === "break"}<br
                        />{:else}{@render marked(seg.marks, body)}{/if}
                {/each}{/if}
        </p>
    {:else}
        <p class="original">
            {#if vm.originalText}{@html vm.originalText}{:else}&nbsp;{/if}
        </p>
    {/if}
</div>

<style>
    .original {
        color: var(--text-untranslated);
    }

    p {
        margin: 0;
    }

    .paragraph-wrapper {
        margin-top: 0;
        margin-bottom: 0.5em;
        display: grid;
        grid-template-columns: 1.5cm auto 1.5cm;
        break-inside: avoid;
        -webkit-column-break-inside: avoid;
    }

    button.translate {
        width: calc(2 * var(--font-size));
        height: calc(2 * var(--font-size));
        padding: 0;
        justify-self: center;
        display: flex;
        align-items: center;
        justify-content: center;
    }

    @media (max-width: 576px) {
        .paragraph-wrapper {
            grid-template-columns: 8mm auto 8mm;
        }

        button.translate {
            --font-size: 12px;
            font-size: var(--font-size);
            width: calc(2 * var(--font-size));
            height: calc(2 * var(--font-size));
        }

        :global(.svelte-fa-base) {
            --font-size: 12px;
            font-size: var(--font-size);
        }
    }
</style>
