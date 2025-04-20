<script lang="ts">
    interface RenditionWithOn extends Rendition {
        on: (event: string, callback: (...args: any) => void) => void;
    }

    import './reader.css'
    import ePub from '../../vendor/epub-js/src/epub'
    import { onMount } from 'svelte';
    import type Rendition from '../../vendor/epub-js/src/rendition';

    import type Contents from '../../vendor/epub-js/src/contents';
    import { extractParagraphs, getSentences, getWords } from './reader';
    import EpubCFI from '../../vendor/epub-js/src/epubcfi';
    import { type DictionaryRequest } from './dictionary';
    import type { Book } from './library';
    import Popup from './Popup.svelte';

    let { book: bookSource, onClose } : {
        book: Book,
        onClose: (e: any) => void
    } = $props();

    let isLoading = $state(true);

    let rendition: RenditionWithOn | null;

    let atStart = $state(false);
    let atEnd = $state(false);

    let viewer = $state<HTMLDivElement | null>(null);

    let popupData: { x:number, y: number, bookSource: Book, request: DictionaryRequest } | null = $state(null);
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
            spread: "none"
        }) as RenditionWithOn;

        rendition.hooks.content.register((contents: Contents) => {
            console.time()
            for (const paragraph of extractParagraphs(contents.content)) {
                if (!paragraph.textContent) {
                    continue;
                }

                setTimeout(() => {
                    for (const sentence of getSentences(paragraph)) {
                        let position = 0;
                        for (const word of getWords(sentence)) {
                            let currentPosition = position;
                            let cfi = new EpubCFI(word, contents.section.cfiBase).toString();
                            rendition?.annotations.append("underline", cfi, {
                                cb: (e: MouseEvent) => {
                                    const target = e.target as Element;
                                    const rect = target.getBoundingClientRect();
                                    contentClickEnabled = false;
                                    popupData = {
                                        x: rect.left,
                                        y: rect.top + rect.height,
                                        bookSource,
                                        request: {
                                            paragraph: paragraph.textContent!,
                                            sentence: sentence.toString(),
                                            word: {
                                                position: currentPosition,
                                                value: word.toString(),
                                            }
                                        }
                                    }
                                    setTimeout(() => {
                                        contentClickEnabled = true;
                                    }, 500);
                                }
                            });
                            position++;
                        }
                    }
                });
            }
            console.timeEnd();
        })

        rendition.on("relocated", (location) => {
            atStart = location.atStart;
            atEnd = location.atEnd;
            popupData = null;
            console.log("Relocated to", location.start.cfi);
            bookSource.updateCfi(location.start.cfi);
        })

        if (bookSource.metadata.lastCfi) {
            console.log("Last CFI", bookSource.metadata.lastCfi);
            await rendition.display(bookSource.metadata.lastCfi);
        } else {
            await rendition.display();
        }
    })
</script>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div id="content" 
    class="ltr"
    dir="ltr"
    role="article"
    onclick="{(e) => {
        if (popupData !== null && contentClickEnabled) popupData = null}
    }">
    <button id="home" onclick="{onClose}">Library</button>
    <div bind:this={viewer} id="viewer" class={["paginated", popupData !== null && "ignore-pointer-events" ]}>
        {#if isLoading}
            <div id="loader"></div>
        {/if}
    </div>
    {#if !atStart}
        <button
            id="prev"
            class="arrow" 
            aria-label="previous"
            onclick="{(e) => {
                rendition?.prev();
                e.preventDefault();
            }}"></button>
    {/if}
    {#if !atEnd}
        <button
            id="next"
            class="arrow"
            aria-label="next"
            onclick="{(e) => {
                rendition?.next();
                e.preventDefault();
            }}"></button>
    {/if}

    {#if popupData}
        <Popup {...popupData} onclose={() => popupData = null} />
    {/if}
</div>

<style>
    .ignore-pointer-events {
        pointer-events: none;
    }
</style>