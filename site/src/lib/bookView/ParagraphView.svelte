<script lang="ts">
    import { getContext } from "svelte";
    import { type Library } from "../library.svelte";
    import { type SentenceWordTranslation } from "../data/db";

    let { paragraphId, sentenceWordId }: {
        paragraphId: number,
        sentenceWordId: number | null,
    } = $props();

    const library: Library = getContext('library');
    const paragraph = $derived(library.getParagraph(paragraphId));

    const wordIdPrefix = "sentence-word-";

    const translationHtml = $derived.by(() => {
        if (!$paragraph || !$paragraph.translation) {
            return "";
        }

        let result = [];
        for (const sentence of $paragraph.translation.sentences) {
            let words = [];
            for (let i = 0; i < sentence.words.length; i++) {
                const word: SentenceWordTranslation = sentence.words[i];
                if (word.isPunctuation) {
                    continue;
                }

                let nextWord: SentenceWordTranslation | null = null;
                if (i < sentence.words.length - 1) {
                    nextWord = sentence.words[i + 1];
                }

                let text;
                if (nextWord && nextWord.isPunctuation) {
                    text = `${word.original}${nextWord.original}`
                } else {
                    text = word.original;
                }

                const additionalClass = word.id === sentenceWordId ? " selected" : ""

                words.push(`<span class="word-span${additionalClass}" id="${wordIdPrefix}${word.id}">${text}</span>`);
            }
            result.push(...words);
        }
        return result.join(" ");
    });
</script>

{#if $paragraph}
{#if !$paragraph.translation}
<p class="original">
    {$paragraph.originalText}
</p>
{:else}
<!-- svelte-ignore a11y_click_events_have_key_events -->
<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<p>
    {@html translationHtml}
</p>
{/if}
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
        margin-bottom: 1.5em;
    }
</style>