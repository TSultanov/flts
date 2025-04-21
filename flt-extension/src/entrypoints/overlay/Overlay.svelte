<script lang="ts">
    import { DictionaryRequest } from "@/lib/dictionary";
    import Popup from "@/lib/Popup.svelte";
    import { on } from "svelte/events";

    const {
        x,
        y,
        position,
        word,
        sentence,
        paragraph,
        onClose,
    }: {
        x: number;
        y: number;
        position: number,
        word: string;
        sentence: string;
        paragraph: string;
        onClose: () => void;
    } = $props();

    let request: DictionaryRequest | null = $state(null);

    function translate() {
        request = {
            paragraph,
            sentence,
            word: {
                position: position,
                value: word,
            },
        };
    }
</script>

{#if word && !request}
    <div id="overlay" style:top={`${y}px`} style:left={`${x}px`}>
        <div id="word">{word}</div>
        <button onclick={translate}>Translate</button>
    </div>
{/if}
{#if request}
    <Popup {x} {y} {request} onclose={onClose} />
{/if}

<style>
    #word {
        text-overflow: ellipsis;
        overflow: hidden;
    }

    #overlay {
        position: absolute;
        background-color: light-dark(white, black);
        color: light-dark(black, white);
        border: 1px solid black;
        display: flex;
        justify-content: space-between;
        align-items: center;
        max-height: 30px;
        min-width: 100px;
        overflow: hidden;
    }
</style>
