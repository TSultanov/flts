<script lang="ts">
    import Fa from "svelte-fa";
    import { faLanguage } from "@fortawesome/free-solid-svg-icons";
    import { getContext } from "svelte";
    import CircularProgress from "../widgets/CircularProgress.svelte";
    import type { Library } from "../data/library";
    import type { ParagraphView } from "../data/sql/book";
    import type { UUID } from "../data/v2/db";
    import { ParagraphViewModel } from "./ParagraphViewModel.svelte";

    let {
        bookId,
        paragraph,
        sentenceWordIdToDisplay = null,
    }: {
        bookId: UUID;
        paragraph: ParagraphView;
        sentenceWordIdToDisplay?: [number, number, number] | null;
    } = $props();

    const library: Library = getContext("library");
    const vm = new ParagraphViewModel(library, () => ({
        bookId,
        paragraph,
        sentenceWordIdToDisplay,
    }));
</script>

<div
    class="paragraph-wrapper"
    data-paragraph-id={paragraph.id}
    bind:this={vm.wrapper}
>
    {#if !vm.translationHtml}
        <button
            class="translate"
            aria-label="Translate paragraph"
            title="Translate paragraph"
            onclick={(e) => vm.translate(!(e.metaKey || e.ctrlKey))}
            disabled={vm.isTranslating}
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
        <p class="original">
            {@html vm.originalText}
        </p>
    {:else}
        <div></div>
        <p>
            {@html vm.translationHtml}
        </p>
    {/if}
</div>

<style>
    :global(.word-span.selected) {
        outline: 1px dotted var(--selected-color);
    }

    :global(.word-span) {
        position: relative;
        display: inline-block;
    }

    :global(.word-span::before) {
        content: attr(data-translation);
        display: none;
        position: absolute;
        left: 0;
        right: 0;
        top: 0;
        width: 100%;
        font-size: var(--word-translation-font-size, 0.55em);
        text-align: center;
        line-height: 1;
        padding: 0.05em 0.1em;
        box-sizing: border-box;
        white-space: nowrap;
        opacity: 0;
        -webkit-user-select: none;
        user-select: none;
        pointer-events: none;
        transition: opacity 150ms ease;
        z-index: 2;
        overflow: hidden;
    }

    :global(.word-span.show-translation[data-translation]::before) {
        display: block;
        opacity: 0.9;
    }

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

    /* iOS/WebKit can struggle when forced to keep long blocks unbroken inside columns. */
    @media (pointer: coarse) {
        .paragraph-wrapper {
            break-inside: auto;
            -webkit-column-break-inside: auto;
        }
    }

    button.translate {
        /* margin-top: 0.5em; */
        width: calc(2 * var(--font-size));
        height: calc(2 * var(--font-size));
        padding: 0;
        justify-self: center;
        display: flex;
        align-items: center;
        justify-content: center;
    }
</style>
