<script lang="ts">
    import { getContext } from "svelte";
    import { Library, } from "./library.svelte";
    import { route } from "@mateothegreat/svelte5-router";

    const library: Library = getContext("library");
    const books = library.getLibraryBooks();
</script>

{#if $books && $books.length > 0}
    <div class="books">
        <h1>Books</h1>
        <ul>
            {#each $books as book}
                <li>
                    <a use:route href="/book/{book.id}">{book.title} - {book.chapters.length} chapter(s)
                        {#if book.translatedParagraphsCount != book.paragraphsCount}
                            - {(book.translatedParagraphsCount / book.paragraphsCount * 100).toFixed(0)}% translated
                        {/if}
                    </a>
                    <button onclick="{() => library.deleteBook(book.id)}">Delete</button>
                </li>
            {/each}
        </ul>
    </div>
{/if}
