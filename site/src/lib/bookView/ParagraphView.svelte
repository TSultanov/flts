<script lang="ts">
    import type { Evolu } from "@evolu/common";
    import type { Books } from "../data/evolu/book";
    import type {
        BookChapterParagraphId,
        BookParagraphTranslationSentenceWordId,
        DatabaseSchema,
    } from "../data/evolu/schema";
    import { getContext } from "svelte";
    import { queryState } from "@evolu/svelte";
    import { decode } from "html-entities";

    let {
        paragraph,
        sentenceWordIdToDisplay,
    }: {
        paragraph: {
            id: BookChapterParagraphId;
            originalHtml: string | null;
            originalText: string | null;
        };
        sentenceWordIdToDisplay: BookParagraphTranslationSentenceWordId | null;
    } = $props();

    const originalText = $derived(
        paragraph.originalHtml ?? paragraph.originalText ?? "<failed to load >",
    );

    const books: Books = getContext("books");
    const evolu: Evolu<DatabaseSchema> = getContext("evolu");

    const paragraphTranslationQuery = $derived(
        books.paragraphTranslation(paragraph.id),
    );

    const paragraphTranslation = queryState(
        evolu,
        () => paragraphTranslationQuery,
    );

    const translationHtml = $derived.by(() => {
        if (paragraphTranslation.rows.length > 0) {
            const result = [];

            let pIdx = 0;
            let sentenceIdx = 0;
            for (const word of paragraphTranslation.rows) {
                if (word.isPunctuation) {
                    continue;
                }

                const w = decode(word.original);
                const len = w.length;
                let offset = 0;
                for (; offset < originalText.length - pIdx; offset++) {
                    const pWord = decode(
                        originalText.slice(pIdx + offset, pIdx + offset + len),
                    );

                    if (w.length <= 2) {
                        if (w.toLowerCase() === pWord.toLowerCase()) {
                            break;
                        }
                    } else if (
                        levenshteinDistance(
                            w.toLowerCase(),
                            pWord.toLowerCase(),
                        ) < 2
                    ) {
                        break;
                    }
                }

                if (offset > 0) {
                    const text = originalText.slice(pIdx, pIdx + offset);
                    result.push(text);
                }

                pIdx += offset;

                const additionalClass =
                    word.wordId === sentenceWordIdToDisplay ? "selected" : "";

                const text = originalText.slice(pIdx, pIdx + len);
                result.push(
                    `<span class="word-span ${additionalClass}" data-sentence="${word.sentenceId}" data-word="${word.wordId}">${text}</span>`,
                );

                pIdx += len;
            }

            if (pIdx < originalText.length) {
                const text = originalText.slice(pIdx, originalText.length);
                result.push(text);
            }

            return result.join("");
        }

        return null;
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
                    track[j - 1][i - 1] + indicator, // substitution
                );
            }
        }
        return track[str2.length][str1.length];
    }
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
