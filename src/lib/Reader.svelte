<script lang="ts">
    interface RenditionWithOn extends Rendition {
        on: (event: string, callback: (...args: any) => void) => void;
    }

    import './reader.css'
    import ePub from '../../vendor/epub-js/src/epub'
    import { onMount } from 'svelte';
    import type Rendition from '../../vendor/epub-js/src/rendition';

    import type Contents from '../../vendor/epub-js/src/contents';
    import { extractParagraphs, getWordRangesFromTextNode } from './reader';
    import EpubCFI from '../../vendor/epub-js/src/epubcfi';
    import { Dictionary, type ParagraphTranslation, type WordTranslation } from './dictionary';
    import { hashBuffer, hashFile } from './utils';
    import { getConfig } from './config';
    import { GoogleGenAI } from '@google/genai';
    import Popup from './Popup.svelte';
    import type { Book } from './library';

    let { book: bookSource, onClose } : {
        book: Book,
        onClose: (e: any) => void
    } = $props();

    let isLoading = $state(true);

    let rendition: RenditionWithOn | null;

    let annotations = new Set<string>();

    let atStart = $state(false);
    let atEnd = $state(false);

    let viewer = $state<HTMLDivElement | null>(null);

    let popupData: { x:number, y: number, sentence: string; translation: WordTranslation } | null = $state(null);

    function* getWordRanges(contents: Contents, node: Node): Generator<string> {
        if (node.nodeType === Node.TEXT_NODE && node.textContent?.trim() != '') {
            let ranges = getWordRangesFromTextNode(node)
            for (let range of ranges) {
                let cfiRange = new EpubCFI(range, contents.section.cfiBase).toString();

                yield cfiRange;
            }
        }

        for (let child of node.childNodes) {
            yield* getWordRanges(contents, child);
        }
    }


    async function annotateWords(rendition: Rendition, contents: Contents, node: Node, translation: ParagraphTranslation) {
        function* translationWords(): Generator<{sentence: string, word: WordTranslation}> {
            for (let sentence of translation.sentences) {
                for (let word of sentence.words) {
                    yield {sentence: sentence.fullTranslation, word}
                }
            }
        }

        let originalGen = getWordRanges(contents, node);
        let translationGen = translationWords();

        let currentOriginalCfi = originalGen.next()
        let currentTranslation = translationGen.next()
        while (!currentOriginalCfi.done && !currentTranslation.done) {
            let currentCfi = currentOriginalCfi.value;
            let cfis = [currentCfi];
            let currentText = (await rendition.book.getRange(currentCfi)).toString();
            let {sentence, word: currentTranslationValue} = currentTranslation.value;

            const skipCharacters = ['!', ',', ';', '?', ':', '"', '’', '“', '”', '(', ')', '[', ']', '{', '}', '…', '—', '–', '•', '·', '•', '°', '\n', '\''];
            let currentTranslationValueOriginal = currentTranslationValue.original.replace(' ', '');
            for (const c of skipCharacters) {
                currentTranslationValueOriginal = currentTranslationValueOriginal.replaceAll(c, '');
                currentText.replaceAll(c, '');
            }

            if (skipCharacters.includes(currentTranslationValueOriginal)) {
                currentTranslation = translationGen.next();
                continue;
            }

            while (currentText.toLowerCase() !== currentTranslationValueOriginal.toLowerCase()) {
                if (currentTranslationValueOriginal.toLowerCase().startsWith(currentText.toLowerCase())) {
                    currentOriginalCfi = originalGen.next();
                    currentCfi = currentOriginalCfi.value;
                    cfis.push(currentCfi);
                    currentText = currentText + (await rendition.book.getRange(currentCfi)).toString();
                } else {
                    break;
                }
            }

            if (currentText.toLowerCase() === currentTranslationValueOriginal.toLowerCase()) {
                for (const cfi of cfis) {
                    if (!annotations.has(cfi)) {
                        rendition.annotations.append('underline', cfi, {
                            cb: async (e: MouseEvent) => {
                                console.log(cfi);

                                const target = e.target as Element;
                                const rect = target.getBoundingClientRect();

                                popupData = {
                                    x: rect.left,
                                    y: rect.top + rect.height,
                                    sentence,
                                    translation: currentTranslationValue
                                };
                            },
                        });
                        annotations.add(cfi);
                    }
                }
            }

            currentOriginalCfi = originalGen.next();
            currentTranslation = translationGen.next();
        }
    }

    onMount(async () => {
        const bookData = await bookSource.getContent();
        if (!bookData) {
            console.error("Failed to read data for book", bookSource);
            return;
        }
        let book = ePub(bookData);

        let config = await getConfig();
        let ai = new GoogleGenAI({apiKey: config.api_key})
        let dictionary = await bookSource.getDictionary(ai);

        await book.opened;

        isLoading = false;

        rendition = book.renderTo("viewer", {
            spread: "none"
        }) as RenditionWithOn;

        rendition.hooks.content.register(async (contents: Contents) => {
            const paragraphs = extractParagraphs(contents.content);
            for (let paragraph of paragraphs) {
                const textContent = paragraph.textContent!.trim();
                const translation = await dictionary.getCachedTranslation(textContent);

                const rect = paragraph.getBoundingClientRect();

                const btn = document.createElement('button');
                btn.style.height = "20px";
                btn.style.position = "absolute";
                btn.style.left = `${rect.left - 50}px`;
                btn.style.top = `${rect.top}px`;

                btn.innerText = "T";

                if (translation) {
                    await annotateWords(rendition!, contents, paragraph, translation);
                }

                paragraph.appendChild(btn);

                btn.onclick = async (e: MouseEvent) => {
                    e.stopPropagation();
                    e.preventDefault();

                    btn.disabled = true;
                    btn.innerText = "W";
                    const translation = await dictionary.translateParagraph(textContent);
                    btn.disabled = false;
                    btn.innerText = "T";
                    if (translation) {
                        console.log(translation);
                        await annotateWords(rendition!, contents, paragraph, translation);
                    }
                }
            }
        })

        rendition.on("relocated", (location) => {
            atStart = location.atStart;
            atEnd = location.atEnd;
            popupData = null;
            bookSource.updateCfi(location.start.cfi);
        })

        if (bookSource.metadata.lastCfi) {
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
    onclick="{(e) => {if (popupData !== null) popupData = null}}">
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