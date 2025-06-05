<script lang="ts">
    import { getContext } from "svelte";
    import { type Library } from "../library.svelte";
    import { type SentenceWordTranslation } from "../data/db";
    import { decode } from 'html-entities';

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

        let pIdx = 0;
        let result = [];
        for (const sentence of $paragraph.translation.sentences) {
            for (const word of sentence.words) {
                if (word.isPunctuation) {
                    continue;
                }

                const w = decode(word.original);
                const len = w.length;
                let offset = 0;
                for (; offset < $paragraph.originalText.length - pIdx; offset++) {
                    const pWord = decode($paragraph.originalText.slice(pIdx+offset, pIdx+offset+len));

                    if (w.length <= 2) {
                        if (w.toLowerCase() === pWord.toLowerCase()) {
                            break;
                        }
                    } else if (levenshteinDistance(w.toLowerCase(), pWord.toLowerCase()) < 2) {
                        break;
                    }
                }

                if (offset > 0) {
                    result.push($paragraph.originalText.slice(pIdx, pIdx+offset));
                }

                pIdx += offset;
                result.push(`<span class="word-span" id="${wordIdPrefix}${word.id}" data="${word.original}" data-offset="${offset}">${$paragraph.originalText.slice(pIdx, pIdx+len)}</span>`);
                pIdx += len;
            }
        }
        if (pIdx < $paragraph.originalText.length) {
            result.push($paragraph.originalText.slice(pIdx, $paragraph.originalText.length));
        }
        return result.join("");
    });

    function levenshteinDistance(str1: string, str2: string) {
        const track = Array(str2.length + 1)
            .fill(null)
            .map(() => Array(str1.length + 1).fill(null));

        for (let i = 0; i <= str1.length; i += 1) {
            track[0][i] = i;
        }
        for (let j = 0; j <= str2.length; j += 1) {
            track[j][0] = j;
        }

        for (let j = 1; j <= str2.length; j += 1) {
            for (let i = 1; i <= str1.length; i += 1) {
            const indicator = str1[i - 1] === str2[j - 1] ? 0 : 1;
            track[j][i] = Math.min(
                track[j][i - 1] + 1, // deletion
                track[j - 1][i] + 1, // insertion
                track[j - 1][i - 1] + indicator // substitution
            );
            }
        }
        return track[str2.length][str1.length];
    }
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

    :global(.word-span) {
        outline: 1px solid lightblue;
    }
</style>