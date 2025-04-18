<script lang="ts">
    import { onMount } from "svelte";
    import type { WordTranslation } from "./dictionary";

    let {
        x,
        y,
        sentence,
        translation,
        onclose
    } : {
        x: number,
        y: number,
        sentence: string,
        translation: WordTranslation,
        onclose?: () => void
    } = $props();

    let isDragging = true;
    let mouseInnerCoordinates = { x: 0, y: 0};
    let popupEl: HTMLDivElement;

    function handleMouseDown(e: MouseEvent) {
        if (e.button === 0) {
            let rect = (e.target as Element).getBoundingClientRect();
            mouseInnerCoordinates.x = e.clientX - rect.x;
            mouseInnerCoordinates.y = e.clientY - rect.y;

            isDragging = true;
            window.addEventListener('mousemove', movePopup);
            window.addEventListener('mouseup', stopDragging);
            e.preventDefault();
        }
    }

    function movePopup(e: MouseEvent) {
        if (isDragging) {
            x = e.clientX - mouseInnerCoordinates.x;
            y = e.clientY - mouseInnerCoordinates.y;
        }
    }

    function stopDragging() {
        isDragging = false;
        window.removeEventListener('mousemove', movePopup);
        window.removeEventListener('mouseup', stopDragging);
    }

    onMount(() => {
        setTimeout(() => {
            if (popupEl) {
                const rect = popupEl.getBoundingClientRect();
                // Adjust if right edge goes off screen
                if (rect.right > window.innerWidth) {
                    x = window.innerWidth - rect.width;
                }
                // Adjust if bottom edge goes off screen
                if (rect.bottom > window.innerHeight) {
                    y = window.innerHeight - rect.height;
                }
            }
        });
    });
</script>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div bind:this={popupEl} id="popup" style:top={`${y}px`} style:left={`${x}px`} onclick="{(e) => e.stopPropagation()}">
    <div class="popup-header">
        <div class="popup-header-title" role="button" tabindex="0" onmousedown={handleMouseDown}>Translation</div>
        <button class="popup-button" aria-label="Close" onclick={onclose}>x</button>
    </div>
    <div class="popup-body">
        <table>
            <tbody>
                <tr>
                    <td>
                        Original
                    </td>
                    <td>
                        {translation.original}
                    </td>
                </tr>
                <tr>
                    <td>
                        Translations
                    </td>
                    <td>
                        <ul>
                            {#each translation.translations as tr}
                                <li>{tr}</li>
                            {/each}
                        </ul>
                    </td>
                </tr>
                {#if translation.note}
                <tr>
                    <td>Note</td>
                    <td>{translation.note}</td>
                </tr>
                {/if}
                <tr>
                    <td>
                        Sentence
                    </td>
                    <td>
                        {sentence}
                    </td>
                </tr>
            </tbody>
        </table>
    </div>
</div>

<style>
    .popup-button {
        user-select: none;
        text-decoration: none;
    }

    .popup-header {
        display: flex;
        justify-content: space-between;
        background-color: lightblue;
        border-bottom: 1px solid black;
        user-select: none;
        text-decoration: none;
    }

    .popup-header-title {
        padding: 0 5px;
        flex-grow: 1;
    }

    .popup-body {
        padding: 0px;
    }

    .popup-body ul {
        margin: 0px;
    }

    #popup {
        border: 1px solid black;
        background-color: white;
        position: absolute;
        width: 400px;
        z-index: 10;
    }

    table {
        border-collapse: collapse;
        width: 100%;
    }

    td {
        border: 1px solid rgb(160 160 160);
        padding: 2px 4px;
    }
</style>