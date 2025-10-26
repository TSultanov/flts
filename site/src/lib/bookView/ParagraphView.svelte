<script lang="ts">
    import type { ParagraphView } from "../data/sql/book";

    let {
        paragraph,
        sentenceWordIdToDisplay,
    }: {
        paragraph: ParagraphView;
        sentenceWordIdToDisplay: [number, number, number] | null;
    } = $props();

    const originalText = $derived(paragraph.original);
    const translationHtml = $derived(paragraph.translation);
</script>

{#if !translationHtml}
    <p class="original">
        {@html originalText}
    </p>
{:else}
    <p>
        {@html translationHtml}
    </p>
{/if}

<style>
    :global(.word-span.selected) {
        outline: 1px dotted var(--selected-color);
    }

    .original {
        color: var(--text-inactive);
    }

    p {
        margin-top: 0;
        margin-bottom: 1em;
    }
</style>
