<script lang="ts">
    import { getContext } from "svelte";
    import { Library } from "../data/library";
    import type { Language } from "../config";
    import { getterToReadableWithEvents } from "../data/tauri";
    import { navigate } from "../../router";

    let title = $state("");
    let text = $state("");
    const languages = getterToReadableWithEvents<Language[]>("get_languages", {}, [], []);
    let sourceLanguageId = $state("eng");

    const canImport = $derived(title.length > 0 && text.length > 0);

    const library: Library = getContext("library");

    async function save() {
        const langId = sourceLanguageId;
        await library.importText(title, text, langId);
        navigate("/library");
    }
</script>

<div class="import-view">
    <label for="title">Title: </label>
    <input type="text" id="title" bind:value={title} />
    <label for="text">Text: </label>
    <textarea id="text" bind:value={text}></textarea>
    <label for="src-lang">Source language:</label>
    <select id="src-lang" bind:value={sourceLanguageId}>
        {#each $languages as l}
            <option value={l.id}>{l.name}{l.localName ? ` (${l.localName})` : ""}</option>
        {/each}
    </select>
    <div class="button">
        <button disabled={!canImport} onclick={save} class="primary">Import</button>
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
        flex: 0 1 auto;
        text-align: right;
    }
</style>
