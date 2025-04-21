<script lang="ts">
    import { onMount } from "svelte";
    import { Dictionary, DictionaryRequest, Translation } from "./dictionary";
    import { getConfig } from "./config";
    import { GoogleGenAI } from "@google/genai";

    let {
        x,
        y,
        request,
        onclose,
    }: {
        x: number;
        y: number;
        request: DictionaryRequest;
        onclose?: () => void;
    } = $props();

    let isDragging = true;
    let mouseInnerCoordinates = { x: 0, y: 0 };
    let popupEl: HTMLDivElement;

    let translation: Translation | null = $state(null);
    let errorMessage: string | null = $state(null);
    let dictionary: Dictionary | null = null;

    function handleTouchStart(e: TouchEvent) {
        const touch = e.touches[0];
        isDragging = true;
        let rect = (e.target as Element).getBoundingClientRect();
        mouseInnerCoordinates.x = touch.clientX - rect.x;
        mouseInnerCoordinates.y = touch.clientY - rect.y;
    }

    function handleTouchEnd(e: TouchEvent) {
        isDragging = false;
    }

    function handleTouchMove(e: TouchEvent) {
        const touch = e.touches[0];
        if (isDragging) {
            x = touch.clientX - mouseInnerCoordinates.x;
            y = touch.clientY - mouseInnerCoordinates.y;
        }
    }

    function handleMouseDown(e: MouseEvent) {
        if (e.button === 0) {
            let rect = (e.target as Element).getBoundingClientRect();
            mouseInnerCoordinates.x = e.clientX - rect.x;
            mouseInnerCoordinates.y = e.clientY - rect.y;

            isDragging = true;
            window.addEventListener("mousemove", movePopup);
            window.addEventListener("mouseup", stopDragging);
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
        window.removeEventListener("mousemove", movePopup);
        window.removeEventListener("mouseup", stopDragging);
    }

    function fixPosition() {
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
    }

    async function refresh() {
        if (dictionary) {
            translation = null;
            translation = await dictionary.getTranslation(request);
        }
    }

    onMount(async () => {
        fixPosition();

        try {
            const config = await getConfig();
            if (!config.apiKey) {
                errorMessage = "API key is not set";
                return;
            }
            if (!config.to) {
                errorMessage = "Target language is not set";
                return;
            }
            const ai = new GoogleGenAI({apiKey: config.apiKey})
            dictionary = new Dictionary(ai, config.to);

            translation = await dictionary.getCachedTranslation(request);
            if (!translation) {
                translation = await dictionary.getTranslation(request);
            }
        } finally {
            fixPosition();
        }
    });
</script>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
    bind:this={popupEl}
    id="popup"
    style:top={`${y}px`}
    style:left={`${x}px`}
    onclick={(e) => e.stopPropagation()}
>
    <div class="popup-header">
        <div
            class="popup-header-title"
            role="button"
            tabindex="0"
            onmousedown={handleMouseDown}
            ontouchstart={handleTouchStart}
            ontouchend={handleTouchEnd}
            ontouchmove={handleTouchMove}
        >
            {request.word.value}
        </div>
        <button class="popup-button" aria-label="Refresh" onclick={refresh}
            >Refresh</button
        >
        <button class="popup-button" aria-label="Close" onclick={onclose}
            >x</button
        >
    </div>
    <div class="popup-body">
        {#if errorMessage}
        <p class="error">{errorMessage}</p>
        {/if}
        {#if translation}
            <table>
                <tbody>
                    <tr>
                        <td> Translations </td>
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
                        <td> Sentence </td>
                        <td>
                            {translation.sentenceTranslation}
                        </td>
                    </tr>
                </tbody>
            </table>
        {:else}
            Loading...
        {/if}
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
        align-items: center;
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
        padding: 0 0 0 20px;
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

    .error {
        color: red;
    }
</style>
