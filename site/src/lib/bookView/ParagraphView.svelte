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

    const openingParens = ["“", "(", "{", "[", "&ldquo;"];

    const translationHtml = $derived.by(() => {
        if (!$paragraph || !$paragraph.translation) {
            return "";
        }

        let result = [];
        for (const sentence of $paragraph.translation.sentences) {
            let words = [];
            let prevPunctuation: string|null = null;
            for (let i = 0; i < sentence.words.length; i++) {
                const word: SentenceWordTranslation = sentence.words[i];
                if (word.isPunctuation && !word.isStandalonePunctuation && !word.isOpeningParenthesis && !openingParens.find(x => x === word.original.trim())) {
                    continue;
                } else if (word.isPunctuation && openingParens.find(x => x === word.original.trim())) {
                    prevPunctuation = word.original;
                    continue;
                }

                let nextWord: SentenceWordTranslation | null = null;
                if (i < sentence.words.length - 1) {
                    nextWord = sentence.words[i + 1];
                }

                if (word.isPunctuation && (word.original === '<br>' || word.original === '<br/>' || word.original === '&lt;br&gt;')) {
                    if (nextWord?.isPunctuation && (nextWord.original === '<br>' || nextWord.original === '<br/>' || nextWord.original === '&lt;br&gt;')) {
                        continue;
                    }
                    words.push("<br>");
                    continue;
                }

                let text;
                if (nextWord 
                    && nextWord.isPunctuation
                    && !nextWord.isStandalonePunctuation
                    && !nextWord.isOpeningParenthesis
                    && !openingParens.find(x => x === nextWord.original.trim())) {
                    text = `${word.original}${nextWord.original}`
                } else {
                    text = word.original;
                }

                if (prevPunctuation) {
                    text = `${prevPunctuation}${text}`
                }

                const additionalClass = word.id === sentenceWordId ? " selected" : ""

                words.push(`<span class="word-span${additionalClass}" id="${wordIdPrefix}${word.id}">${text}</span>`);
                prevPunctuation = null;
            }
            result.push(...words);
        }
        return result.join(" ");
    });
</script>

{#if $paragraph}
{#if !$paragraph.translation}
<p class="original">
    {@html $paragraph.originalText}
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