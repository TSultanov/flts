<script lang="ts">
    import { getContext } from "svelte";
    import { goto } from "@mateothegreat/svelte5-router";
    import type { Books } from "../data/evolu/book";
    import type { TranslationQueue } from "../data/queueDb";

    let title = $state("");
    let text = $state("");

    const canImport = $derived(title.length > 0 && text.length > 0);

    const books: Books = getContext("books");
    const translationQueue: TranslationQueue = getContext("translationQueue");

    async function save() {
        const id = books.createBookFromText(title, text);
        translationQueue.scheduleFullBookTranslation(id);
        goto("/library");
    }
</script>

<div class="import-view">
    <label for="title">Title: </label>
    <input type="text" id="title" bind:value={title} />
    <label for="text">Text: </label>
    <textarea id="text" bind:value={text}></textarea>
    <div class="button">
        <button disabled={!canImport} onclick={save} class="primary"
            >Import</button
        >
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
