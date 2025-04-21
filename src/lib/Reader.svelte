<script lang="ts">
    interface RenditionWithOn extends Rendition {
        on: (event: string, callback: (...args: any) => void) => void;
    }

    import "./reader.css";
    import ePub from "../../vendor/epub-js/src/epub";
    import { onMount } from "svelte";
    import type Rendition from "../../vendor/epub-js/src/rendition";

    import type Contents from "../../vendor/epub-js/src/contents";
    import { extractParagraphs, getSentences, getWords } from "./reader";
    import { Dictionary, type DictionaryRequest } from "./dictionary";
    import type { Book } from "./library";
    import Popup from "./Popup.svelte";
    import { getConfig } from "./config";
    import { GoogleGenAI } from "@google/genai";

    let {
        book: bookSource,
        onClose,
    }: {
        book: Book;
        onClose: (e: any) => void;
    } = $props();

    let isLoading = $state(true);

    let rendition: RenditionWithOn | null;

    let atStart = $state(false);
    let atEnd = $state(false);

    let viewer = $state<HTMLDivElement | null>(null);

    let popupData: {
        x: number;
        y: number;
        dictionary: Dictionary;
        request: DictionaryRequest;
    } | null = $state(null);
    let contentClickEnabled = true; // FIXME: Without this flag the popup will open and immediately close on iPad, as #content element registers click for some reason

    onMount(async () => {
        const bookData = await bookSource.getContent();
        if (!bookData) {
            console.error("Failed to read data for book", bookSource);
            return;
        }
        let book = ePub(bookData);

        await book.opened;

        isLoading = false;

        rendition = book.renderTo("viewer", {
            spread: "none",
        }) as RenditionWithOn;
        const viewer = document.querySelector("#viewer");

        let config = await getConfig();
        let ai = new GoogleGenAI({ apiKey: config.api_key });
        let dictionary = await bookSource.getDictionary(ai);

        rendition.hooks.content.register((contents: Contents) => {
            console.time();
            for (const paragraph of extractParagraphs(contents.content)) {
                if (!paragraph.textContent) {
                    continue;
                }

                const structure: Array<{
                    sentence: Range;
                    words: Array<{ position: number; range: Range }>;
                }> = [];

                for (const sentence of getSentences(paragraph)) {
                    let position = 0;
                    let s = {
                        sentence,
                        words: new Array(),
                    };
                    for (const word of getWords(sentence)) {
                        s.words.push({
                            position,
                            range: word,
                        });
                        position++;
                    }
                    structure.push(s);
                }

                paragraph.addEventListener("click", (ev: Event) => {
                    console.time("paragraph");
                    const e = ev as MouseEvent;

                    try {
                        for (const sentence of structure) {
                            for (const word of sentence.words) {
                                for (const rect of word.range.getClientRects()) {
                                    if (
                                        e.clientX >= rect.left &&
                                        e.clientX <= rect.right &&
                                        e.clientY >= rect.top &&
                                        e.clientY <= rect.bottom
                                    ) {
                                        contentClickEnabled = false;

                                        const viewContainer = viewer!.querySelector('.view-container');
                                        const viewRect = viewContainer!.getBoundingClientRect();
                                        const offsetX = viewRect.left;
                                        const offsetY = viewRect.top;

                                        popupData = {
                                            x: rect.left + offsetX,
                                            y: rect.bottom + offsetY,
                                            dictionary,
                                            request: {
                                                paragraph:
                                                    paragraph.textContent!,
                                                sentence:
                                                    sentence.sentence.toString(),
                                                word: {
                                                    position: word.position,
                                                    value: word.range.toString(),
                                                },
                                            },
                                        };
                                        setTimeout(() => {
                                            contentClickEnabled = true;
                                        }, 500);
                                        return;
                                    }
                                }
                            }
                        }
                    } finally {
                        console.timeEnd("paragraph");
                    }
                });
            }
            console.timeEnd();
        });

        rendition.on("relocated", (location) => {
            atStart = location.atStart;
            atEnd = location.atEnd;
            popupData = null;
            console.log("Relocated to", location.start.cfi);
            bookSource.updateCfi(location.start.cfi);
        });

        if (bookSource.metadata.lastCfi) {
            console.log("Last CFI", bookSource.metadata.lastCfi);
            await rendition.display(bookSource.metadata.lastCfi);
        } else {
            await rendition.display();
        }
    });
</script>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div
    id="content"
    class="ltr"
    dir="ltr"
    role="article"
    onclick={(e) => {
        if (popupData !== null && contentClickEnabled) popupData = null;
    }}
>
    <button id="home" onclick={onClose}>Library</button>
    <div
        bind:this={viewer}
        id="viewer"
        class={["paginated", popupData !== null && "ignore-pointer-events"]}
    >
        {#if isLoading}
            <div id="loader"></div>
        {/if}
    </div>
    {#if !atStart}
        <button
            id="prev"
            class="arrow"
            aria-label="previous"
            onclick={(e) => {
                rendition?.prev();
                e.preventDefault();
            }}
        ></button>
    {/if}
    {#if !atEnd}
        <button
            id="next"
            class="arrow"
            aria-label="next"
            onclick={(e) => {
                rendition?.next();
                e.preventDefault();
            }}
        ></button>
    {/if}

    {#if popupData}
        <Popup {...popupData} onclose={() => (popupData = null)} />
    {/if}
</div>

<style>
    .ignore-pointer-events {
        pointer-events: none;
    }
</style>
