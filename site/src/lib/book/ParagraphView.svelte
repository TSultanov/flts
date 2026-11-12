<script lang="ts">
    import Fa from "svelte-fa";
    import { faLanguage } from "@fortawesome/free-solid-svg-icons";
    import { getContext } from "svelte";
    import CircularProgress from "../widgets/CircularProgress.svelte";
    import type { Library } from "../data/library";
    import type { UUID } from "../data/uuid";
    import { ParagraphViewModel } from "./ParagraphViewModel.svelte";
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
        onReady,
    }: {
        bookId: UUID;
        chapterId: number;
        paragraphId: number;
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

<!-- A paragraph with no translation yet, where the reader is: its text and
     the button that asks for one. A translated paragraph is drawn by
     ChapterView from the backend's HTML. -->
<div class="paragraph-wrapper" data-paragraph-id={paragraphId}>
    {#if vm.isUntranslated}
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
    {/if}
    <p class="original">
        {#if vm.originalText}{@html vm.originalText}{:else}&nbsp;{/if}
    </p>
</div>

<style>
    /* The wrapper and its paragraph are styled by ChapterView, which also
       draws paragraphs without this component. */
    button.translate {
        position: absolute;
        top: 0;
        left: calc((var(--paragraph-gutter) - 2 * var(--font-size)) / 2);
        width: calc(2 * var(--font-size));
        height: calc(2 * var(--font-size));
        padding: 0;
        display: flex;
        align-items: center;
        justify-content: center;
    }

    @media (max-width: 576px) {
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
