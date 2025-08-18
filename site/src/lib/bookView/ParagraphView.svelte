<script lang="ts">
    import { sqlBooks, type Paragraph } from '../data/sql/book';
    import type { UUID } from '../data/v2/db';
    import { onMount } from 'svelte';

    let { paragraph, sentenceWordIdToDisplay }: {
        paragraph: Paragraph,
        sentenceWordIdToDisplay: UUID | null,
    } = $props();

    const originalText = $derived(paragraph.originalHtml ?? paragraph.originalText);
    const translationPromise = $derived(sqlBooks.getParagraphTranslationShort(paragraph.uid, "ignoredTODO" as UUID));

    let translationHtml: string | null = $state(null);

    onMount(() => {
        translationPromise.then(translation => {
            if (translation && translation.translationJson) {
                const result = [];

                for (const w of translation.translationJson) {
                    if (w.meta) {
                        const additionalClass = w.meta.wordTranslationUid === sentenceWordIdToDisplay ? "selected" : "";
                        result.push(`<span class="word-span ${additionalClass}" data-paragraph="${paragraph.uid}" data-sentence="${w.meta.sentenceTranslationUid}" data-word="${w.meta.wordTranslationUid}">${w.text}</span>`);
                    } else {
                        result.push(w.text);
                    }
                }

                translationHtml = result.join("");
            }
        })
    })
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