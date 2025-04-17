<script lang="ts">
    import type { WordTranslation } from "./dictionary";

    let {
        x,
        y,
        sentence,
        translation,
    } : {
        x: number,
        y: number,
        sentence: string,
        translation: WordTranslation,
    } = $props();

    $effect(() => {
        console.log(x, y);
    })
</script>

<div id="popup" style:top={`${y}px`} style:left={`${x}px`}>
    <p>Original: {translation.original}</p>
    <p>Translations:</p>
    <ul>
        {#each translation.translations as tr}
            <li>{tr}</li>
        {/each}
    </ul>
    {#if translation.note}
        <p>Note: {translation.note}</p>
    {/if}
    <p>Sentence: {sentence}</p>
</div>

<style>
    #popup {
        border: 1px solid black;
        background-color: white;
        position: absolute;
        max-width: 30%;
    }
</style>