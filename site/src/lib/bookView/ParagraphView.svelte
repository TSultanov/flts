<script lang="ts">
    import { getContext, onMount } from "svelte";
    import type { Library, LibraryBookParagraph, LibrarySentenceWordTranslation } from "../library.svelte";
    import type { MouseEventHandler } from "svelte/elements";

    const { paragraphId }: {
        paragraphId: number
    } = $props();

    let paragraph: LibraryBookParagraph | null = $state(null);
    
    const library: Library = getContext('library');

    onMount(async () => {
        paragraph = await library.getParagraph(paragraphId);
    });

    const translationHtml = $derived.by(() => {
        if (!paragraph || !paragraph.translation) {
            return "";
        }

        let result = [];
        for (const sentence of paragraph.translation.sentences) {
            let words = [];
            for (let i = 0; i < sentence.words.length; i++) {
                const word: LibrarySentenceWordTranslation = sentence.words[i];
                if (word.isPunctuation) {
                    continue;
                }

                let nextWord: LibrarySentenceWordTranslation | null = null;
                if (i < sentence.words.length - 1) {
                    nextWord = sentence.words[i + 1];
                }

                let text;
                if (nextWord && nextWord.isPunctuation) {
                    text = `${word.original}${nextWord.original}`
                } else {
                    text = word.original;
                }
                words.push(`<span class="word-span" id="sentence-word-${word.id}">${text}</span>`);
            }
            result.push(...words);
        }
        return result.join(" ");
    });

    function paragraphClick(e: MouseEvent) {
        const target = document.elementFromPoint(e.clientX, e.clientY);
        if (target && target.classList.contains("word-span")) {
            console.log(target.id);
        }
    }
</script>

{#if paragraph}
{#if !paragraph.translation}
<p class="original">
    {paragraph.originalText}
</p>
{:else}
<!-- svelte-ignore a11y_click_events_have_key_events -->
<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<p onclick={paragraphClick}>
    {@html translationHtml}
</p>
{/if}
{/if}

<style>
    .original {
        color: var(--text-inactive);
    }

    p {
        margin-top: 0;
        margin-bottom: 1.5em;
    }
</style>