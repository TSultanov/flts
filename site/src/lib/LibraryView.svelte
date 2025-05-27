<script lang="ts">
    import { getContext, onMount } from "svelte";
    import { Library, } from "./library.svelte";

    const library: Library = getContext("library");
    const books = $derived(library.libraryBooks);
    $inspect(books);
</script>

{#if books && books.length > 0}
    <div class="books">
        <h1>Books</h1>
        <ul>
            {#each books as book}
                <li>
                    {book.id} - {book.title} - {book.chapters.length} chapter(s)
                    <button onclick="{() => library.deleteBook(book.id)}">Delete</button>
                </li>
            {/each}
        </ul>
    </div>
{/if}
