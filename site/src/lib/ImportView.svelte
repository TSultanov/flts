<script lang="ts">
    import { getContext } from "svelte";
    import { Library } from "./library.svelte";

    let title = $state("");
    let text = $state("");

    const canImport = $derived(title.length > 0 && text.length > 0);
    let importing = $state(false);

    const library: Library = getContext("library");

    async function save() {
        // job = await library.addText(title, text);
    }
</script>

<div class="import-view">
    <label for="title">Title: </label>
    <input type="text" id="title" bind:value={title} />
    <label for="text">Text: </label>
    <textarea id="text" bind:value={text}></textarea>

    <button disabled={!canImport} onclick={save}>Import</button>
</div>

<style>
    .import-view {
        display: grid;
        grid-auto-columns: auto;
        grid-auto-rows: auto auto auto 1fr auto;
        gap: 10px;
        max-width: 100%;
        margin: 0 80px 0 80px;
        height: 100%;
    }
</style>
