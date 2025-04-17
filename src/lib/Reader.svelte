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
    import { getWordRangesFromTextNode } from './reader';
    import EpubCFI from '../../vendor/epub-js/src/epubcfi';

    let isLoading = $state(true);

    let rendition: RenditionWithOn | null = $state(null);

    let atStart = $state(false);
    let atEnd = $state(false);

    function annotateWords(rendition: Rendition, contents: Contents, node: Node) {
        if (node.nodeType === Node.TEXT_NODE && node.textContent?.trim() != '') {
            let ranges = getWordRangesFromTextNode(node)
            let cfiRanges = ranges.map((range) => new EpubCFI(range, contents.section.cfiBase).toString())
            for (let cfiRange of cfiRanges) {
                rendition.annotations.append('underline', cfiRange, {
                    cb: async (e: any) => console.log((await rendition.book.getRange(cfiRange)).toString()),
                });
            }
        }
        for (let child of node.childNodes) {
            annotateWords(rendition, contents, child);
        }
    }

    function annotateEachWord(rendition: Rendition, contents: Contents) {
        annotateWords(rendition, contents, contents.content);
    }

    onMount(async () => {
        let book = ePub(alice);

        await book.opened;

        isLoading = false;

        rendition = book.renderTo("viewer", {
            spread: "none"
        }) as RenditionWithOn;

        rendition.hooks.content.register((e: any) => {
            annotateEachWord(rendition!, e);
        })

        rendition.on("relocated", (location) => {
            atStart = location.atStart;
            atEnd = location.atEnd;
        })

        rendition.on("selected", async (cfiRange, e) => {
            console.log("A", cfiRange)
            let range = await book.getRange(cfiRange);
            console.log("A", range)
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

