<script lang="ts">
    import { getContext } from "svelte";
    import { Library } from "../library.svelte";
    import { goto } from "@mateothegreat/svelte5-router";

    let title = $state("");
    let text = $state("");

    const canImport = $derived(title.length > 0 && text.length > 0);

    const library: Library = getContext("library");

    async function save() {
        await library.importText(title, text);
        goto("/library");
    }
</script>

<div class="import-view">
    <label for="title">Title: </label>
    <input type="text" id="title" bind:value={title} />
    <label for="text">Text: </label>
    <textarea id="text" bind:value={text}></textarea>
    <div class="button">
        <button disabled={!canImport} onclick={save}>Import</button>
    </div>
</div>

<style>
    .import-view {
        display: flex;
        flex-direction: column;
        gap: 10px;
        max-width: 100%;
        height: 100%;
        align-items: stretch;
    }

    #text {
        flex: 1 0 auto;
        resize: vertical;
    }

    .button {
        flex: 0 1 25px;
        text-align: right;

        & button {
            height: 100%;
        }
    }
</style>
