<script lang="ts">
    import { Book, Library } from "./library";
    import type { ReaderState } from "./screens";

    import alice from "../../vendor/epub-js/assets/alice.epub?no-inline";
    import { onMount } from "svelte";

    let {onBookSelect} : {
        onBookSelect: (e: ReaderState) => void
    } = $props();

    let files : FileList | null | undefined = $state();

    let disabled = $derived(!files)

    let library: Library | null = null;
    let catalog: Book[] = $state([]);

    async function addBook() {
        if (files && library) {
            const file = files[0];
            const content = await file.arrayBuffer();
            await library.addBook(content);
            catalog = await library.getCatalog();
            files = null;
        }
    }

    onMount(async () => {
        library = await Library.build();
        catalog = await library.getCatalog();
    })
</script>

<div>
    {#if catalog.length > 0}
    <p>Library</p>
        <ul>
        {#each catalog as book}
            <li>{book.metadata.author} - {book.metadata.title} <button onclick="{(e) => onBookSelect({ book })}">Open</button></li>
        {/each}
        </ul>
    {/if}
    <div>
        <input type="file" bind:files accept=".epub">
        <button onclick="{addBook}" disabled="{disabled}">Add new book</button>
    </div>
</div>