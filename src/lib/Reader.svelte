<script lang="ts">
    interface RenditionWithOn extends Rendition {
        on: (event: string, callback: (...args: any) => void) => void;
    }

    import './reader.css'
    import ePub from '../../vendor/epub-js/src/epub'
    import { onMount } from 'svelte';
    import type Rendition from '../../vendor/epub-js/src/rendition';

    import alice from '../../vendor/epub-js/assets/alice.epub?no-inline';

    let isLoading = $state(true);

    let rendition: RenditionWithOn | null = $state(null);

    let atStart = $state(false);
    let atEnd = $state(false);

    function getWordAtPoint(elem: HTMLElement | Node, x: number, y: number): string | null {
        if (!elem) return null;
        if (elem.nodeType === Node.TEXT_NODE) {
            const text = elem.textContent || "";
            if (!text) return null;
            const range = elem.ownerDocument?.createRange();
            if (!range) return null;
            range.selectNodeContents(elem);
            const endPos = text.length;
            for (let currentPos = 0; currentPos < endPos; currentPos++) {
                range.setStart(elem, currentPos);
                range.setEnd(elem, currentPos + 1);
                const rect = range.getBoundingClientRect();
                if (rect.left <= x && rect.right >= x &&
                    rect.top <= y && rect.bottom >= y) {
                    // Found the character at the point.
                    // Expand to cover the word using a simple regex-based check.
                    let start = currentPos;
                    let end = currentPos + 1;
                    while (start > 0 && /\w/.test(text[start - 1])) {
                        start--;
                    }
                    while (end < text.length && /\w/.test(text[end])) {
                        end++;
                    }
                    range.setStart(elem, start);
                    range.setEnd(elem, end);
                    const ret = range.toString();
                    range.detach();
                    return ret;
                }
            }
            range.detach();
            return null;
        } else {
            for (let i = 0; i < elem.childNodes.length; i++) {
                const word = getWordAtPoint(elem.childNodes[i], x, y);
                if (word) return word;
            }
            return null;
        }
    }

    onMount(async () => {
        let book = ePub(alice);

        await book.opened;

        isLoading = false;

        rendition = book.renderTo("viewer", {
            spread: "none"
        }) as RenditionWithOn;

        rendition.on("relocated", (location) => {
            atStart = location.atStart;
            atEnd = location.atEnd;
        })

        rendition.on("click", (e) => {
            console.log(getWordAtPoint(e.target, e.x, e.y));
        })

        rendition.on("selected", async (cfiRange, contents) => {
            const range = (await book.getRange(cfiRange)).toString();
            console.log(cfiRange, range, contents);
        });

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

