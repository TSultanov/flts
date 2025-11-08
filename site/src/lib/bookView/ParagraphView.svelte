<script lang="ts">
    import Fa from "svelte-fa";
    import {
        faLanguage,
        faArrowsRotate,
    } from "@fortawesome/free-solid-svg-icons";
    import type { ParagraphView } from "../data/sql/book";
    import { getContext, onDestroy } from "svelte";
    import type { Library } from "../data/library";
    import type { UUID } from "../data/v2/db";
    import { listen, type UnlistenFn } from "@tauri-apps/api/event";

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

    let translationRequestId: number | null = $state(null);
    let unsub: UnlistenFn | null = null;

    onDestroy(() => {
        if (unsub) {
            unsub();
        }
    });

    async function listenToTranslationRequestChanges() {
        if (translationRequestId !== null) {
            console.log(
                `Listening for translation request ${translationRequestId}`,
            );
            unsub = await listen<number>(
                "translation_request_complete",
                (cb) => {
                    if (translationRequestId === cb.payload) {
                        translationRequestId = null;
                    }
                },
            );
        }
    }

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

        library
            .getParagraphTranslationRequestId(bookId, paragraph.id)
            .then((id) => {
                translationRequestId = id;
                listenToTranslationRequestChanges();
            });
    });

    async function translateParagraph(event: MouseEvent) {
        const useCache = !(event.metaKey || event.ctrlKey);

        translationRequestId = await library.translateParagraph(
            bookId,
            paragraph.id,
            undefined,
            useCache
        );

        await listenToTranslationRequestChanges();
    }
</script>

<div class="paragraph-wrapper">
    {#if !translationHtml}
        <button
            class="translate"
            aria-label="Translate paragraph"
            title="Translate paragraph"
            onclick={translateParagraph}
            disabled={translationRequestId !== null}
        >
            {#if translationRequestId !== null}
                <div class="spin">
                    <Fa icon={faArrowsRotate} />
                </div>
            {:else}
                <Fa icon={faLanguage} />
            {/if}
        </button>
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
    }

    button.translate {
        /* margin-top: 0.5em; */
        width: calc(2 * var(--font-size));
        height: calc(2 * var(--font-size));
        padding: 0;
        justify-self: center;
    }

    @keyframes spin {
        from {
            transform: rotate(0deg);
        }
        to {
            transform: rotate(360deg);
        }
    }

    .spin {
        animation: spin 2s linear infinite;
    }
</style>
