<script lang="ts">
    import Fa from "svelte-fa";
    import { faLanguage } from "@fortawesome/free-solid-svg-icons";
    import { getContext } from "svelte";
    import CircularProgress from "../widgets/CircularProgress.svelte";
    import type { Library } from "../data/library";
    import type { UUID } from "../data/uuid";
    import { ParagraphViewModel, type WordSelection } from "./ParagraphViewModel.svelte";
    import WordSpan from "./WordSpan.svelte";
    import {
        CHAPTER_STORE_KEY,
        type ChapterParagraphsStore,
    } from "./ChapterParagraphsStore.svelte";

    let {
        bookId,
        paragraphId,
        selection = null,
        mounted = true,
        onWordClick,
        onReady,
    }: {
        bookId: UUID;
        paragraphId: number;
        selection?: WordSelection | null;
        mounted?: boolean;
        onWordClick: (info: {
            paragraphId: number;
            sentence: number;
            word: number;
            flatIndex: number;
        }) => void;
        onReady?: () => void;
    } = $props();

    const library: Library = getContext("library");
    const store: ChapterParagraphsStore = getContext(CHAPTER_STORE_KEY);
    const vm = new ParagraphViewModel(library, store, {
        get bookId() { return bookId; },
        get paragraphId() { return paragraphId; },
        get selection() { return selection; },
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

<div
    class="paragraph-wrapper"
    data-paragraph-id={paragraphId}
>
    {#if mounted && !vm.segments}
        <button
            class="translate"
            aria-label="Translate paragraph"
            title="Translate paragraph"
            onclick={(e) => vm.translate(!(e.metaKey || e.ctrlKey))}
            disabled={vm.isTranslating || !vm.originalText}
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
    {#if mounted && vm.segments}
        <p>
            {#each vm.segments as seg, i (i)}
                {#if seg.kind === "gap"}{@html seg.html}{:else}<WordSpan
                        text={seg.text}
                        sentence={seg.sentence}
                        word={seg.word}
                        flatIndex={seg.flatIndex}
                        translation={seg.translation}
                        manualToggle={vm.visibleWordsSet.has(seg.flatIndex)}
                        familiarity={seg.familiarity}
                        selected={vm.isSelected(seg.sentence, seg.word)}
                        onClick={(w) =>
                            onWordClick({ paragraphId, ...w })}
                    />{/if}
            {/each}
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
</style>
