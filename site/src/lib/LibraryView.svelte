<script lang="ts">
    import { getContext, onMount } from "svelte";
    import { Library, } from "./library.svelte";

    const library: Library = getContext("library");
    const books = library.$libraryBooks;
</script>

{#if books && books.length > 0}
    <div class="books">
        <h1>Books</h1>
        <ul>
            {#each books as book}
                <li>
                    {book.id} - {book.title} - {book.chapters.length} chapter(s)
                    <button>Delete</button>
                </li>
            {/each}
        </ul>
    </div>
{/if}
