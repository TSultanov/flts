<script lang="ts">
    import { decode } from 'html-entities';
    import type { IParagraphView, TranslatedWordId } from "../data/v2/book.svelte";

    let { paragraph, sentenceWordIdToDisplay }: {
        paragraph: IParagraphView,
        sentenceWordIdToDisplay: TranslatedWordId | null,
    } = $props();

    const originalText = $derived(paragraph.original);
    const translation = $derived(paragraph.translationStore);

    const translationHtml = $derived.by(() => {
        if (!paragraph || !paragraph.translation || !$translation) {
            return "";
        }
      
        let pIdx = 0;
        let result = [];
        let sentenceIdx = 0;
        for (const sentence of $translation.sentences) {
            let wordIdx = 0;
            for (const word of sentence.words) {
                if (word.isPunctuation) {
                    wordIdx++;
                    continue;
                }

                const w = decode(word.original);
                const len = w.length;
                let offset = 0;
                for (; offset < originalText.length - pIdx; offset++) {
                    const pWord = decode(originalText.slice(pIdx+offset, pIdx+offset+len));

                    if (w.length <= 2) {
                        if (w.toLowerCase() === pWord.toLowerCase()) {
                            break;
                        }
                    } else if (levenshteinDistance(w.toLowerCase(), pWord.toLowerCase()) < 2) {
                        break;
                    }
                }

                if (offset > 0) {
                    result.push(originalText.slice(pIdx, pIdx+offset));
                }

                pIdx += offset;
                const additionalClass = (
                    sentenceWordIdToDisplay?.chapter === paragraph.id.chapter && 
                    sentenceWordIdToDisplay?.paragraph === paragraph.id.paragraph && 
                    sentenceWordIdToDisplay?.sentence === sentenceIdx &&
                    sentenceWordIdToDisplay?.word === wordIdx
                ) ? "selected" : "";
                result.push(`<span class="word-span ${additionalClass}" data-chapter="${paragraph.id.chapter}" data-paragraph="${paragraph.id.paragraph}" data-sentence="${sentenceIdx}" data-word="${wordIdx}" data="${word.original}" data-offset="${offset}">${originalText.slice(pIdx, pIdx+len)}</span>`);
                pIdx += len;
                wordIdx++;
            }
            sentenceIdx++;
        }
        if (pIdx < originalText.length) {
            result.push(originalText.slice(pIdx, originalText.length));
        }

        const html = result.join("");
        return html;
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

{#if !$translation}
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