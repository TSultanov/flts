<script lang="ts">
    interface RenditionWithOn extends Rendition {
        on: (event: string, callback: (...args: any) => void) => void;
    }

    import './reader.css'
    import ePub from '../../vendor/epub-js/src/epub'
    import { onMount } from 'svelte';
    import type Rendition from '../../vendor/epub-js/src/rendition';

    import alice from '../../vendor/epub-js/assets/alice.epub?no-inline';
    import type Contents from '../../vendor/epub-js/src/contents';
    import { extractParagraphs, getWordRangesFromTextNode } from './reader';
    import EpubCFI from '../../vendor/epub-js/src/epubcfi';

    let isLoading = $state(true);

    let rendition: RenditionWithOn | null = $state(null);

    let annotations = new Set<string>();

    let atStart = $state(false);
    let atEnd = $state(false);

    function annotateWords(rendition: Rendition, contents: Contents, node: Node) {
        if (node.nodeType === Node.TEXT_NODE && node.textContent?.trim() != '') {
            let ranges = getWordRangesFromTextNode(node)
            let cfiRanges = ranges.map((range) => new EpubCFI(range, contents.section.cfiBase).toString())
            for (let cfiRange of cfiRanges) {
                if (!annotations.has(cfiRange)) {
                    rendition.annotations.append('underline', cfiRange, {
                        cb: async (e: any) => console.log((await rendition.book.getRange(cfiRange)).toString()),
                    });
                    annotations.add(cfiRange);
                }
            }
        }
        for (let child of node.childNodes) {
            annotateWords(rendition, contents, child);
        }
    }

    onMount(async () => {
        let book = ePub(alice);

        await book.opened;

        isLoading = false;

        rendition = book.renderTo("viewer", {
            spread: "none"
        }) as RenditionWithOn;

        rendition.hooks.content.register((e: Contents) => {
            let paragraphs = extractParagraphs(e.content);
            console.log(paragraphs);
            for (let paragraph of paragraphs) {
                annotateWords(rendition!, e, paragraph);
            }
        })

        rendition.on("relocated", (location) => {
            atStart = location.atStart;
            atEnd = location.atEnd;
        })

        rendition.on("selected", async (cfiRange, e) => {
            let range = await book.getRange(cfiRange);
        })

        await rendition.display(6);
    })
</script>

<main>
    <div id="content" class="ltr" dir="ltr">
        <div id="viewer" class="paginated">
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
    </div>
</main>

