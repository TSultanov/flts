<script lang="ts">
    import Fa from "svelte-fa";
    import { faLanguage } from "@fortawesome/free-solid-svg-icons";
    import type { ParagraphView } from "../data/sql/book";
    import { getContext } from "svelte";
    import type { Library } from "../data/library";
    import type { UUID } from "../data/v2/db";

    let {
        bookId,
        paragraph,
        sentenceWordIdToDisplay,
    }: {
        bookId: UUID;
        paragraph: ParagraphView;
        sentenceWordIdToDisplay: [number, number, number] | null;
    } = $props();

    const originalText = $derived(paragraph.original);
    const translationHtml = $derived(paragraph.translation);

    const library: Library = getContext("library");

    $effect(() => {
        const selectedElements = document.querySelectorAll(
            ".word-span.selected",
        );
        selectedElements.forEach((el) => {
            el.classList.remove("selected");
        });
        if (sentenceWordIdToDisplay) {
            let element = document.querySelector(
                `.word-span[data-paragraph="${sentenceWordIdToDisplay[0]}"][data-sentence="${sentenceWordIdToDisplay[1]}"][data-word="${sentenceWordIdToDisplay[2]}"]`,
            );
            if (element) {
                element.classList.add("selected");
            }
        }
    });
</script>

<div class="paragraph-wrapper">
    {#if !translationHtml}
        <button
            class="translate"
            aria-label="Translate paragraph"
            onclick={() => library.transalteParagraph(bookId, paragraph.id)}
            ><Fa icon={faLanguage} /></button
        >
        <p class="original">
            {@html originalText}
        </p>
    {:else}
        <div></div>
        <p>
            {@html translationHtml}
        </p>
    {/if}
</div>

<style>
    :global(.word-span.selected) {
        outline: 1px dotted var(--selected-color);
    }

    .original {
        color: var(--text-inactive);
    }

    p {
        margin: 0;
    }

    .paragraph-wrapper {
        margin-top: 0;
        margin-bottom: 0.5em;
        display: grid;
        grid-template-columns: 1.5cm auto 1.5cm;
    }

    button.translate {
        /* margin-top: 0.5em; */
        width: calc(2 * var(--font-size));
        height: calc(2 * var(--font-size));
        padding: 0;
        justify-self: center;
    }
</style>
