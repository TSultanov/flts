<script lang="ts">
    import { getContext } from "svelte";
    import { Library, type LibraryFolder, type LibraryBook } from "./library.svelte";
    import { route } from "@mateothegreat/svelte5-router";
    import ConfirmDialog from "./ConfirmDialog.svelte";

    const library: Library = getContext("library");
    const rootFolder = library.getLibraryBooks();

    let showDeleteDialog = $state(false);
    let bookToDelete: LibraryBook | null = $state(null);

    function requestDeleteBook(book: LibraryBook) {
        bookToDelete = book;
        showDeleteDialog = true;
    }

    function confirmDeleteBook() {
        if (bookToDelete) {
            library.deleteBook(bookToDelete.id);
            bookToDelete = null;
        }
    }

    function cancelDeleteBook() {
        bookToDelete = null;
    }
</script>

{#if $rootFolder}
    <h1>Books</h1>
    {@render FolderComponent($rootFolder)}
{/if}

<ConfirmDialog 
    bind:isOpen={showDeleteDialog}
    title="Delete Book"
    message={bookToDelete ? `Are you sure you want to delete "${bookToDelete.title}"? This action cannot be undone.` : ""}
    onConfirm={confirmDeleteBook}
    onCancel={cancelDeleteBook}
/>

<!-- Recursive folder component snippet -->
{#snippet FolderComponent(folder: LibraryFolder)}
    <details open>
        {#if folder.name}
            <summary>{folder.name}</summary>
        {:else}
            <summary></summary>
        {/if}
        <div>
            <!-- Books in this folder -->
            {#if folder.books.length > 0}
                <ul>
                    {#each folder.books as book}
                        <li>
                            <a use:route href="/book/{book.id}">{book.title} - {book.chapters.length} chapter(s)
                                {#if book.translatedParagraphsCount != book.paragraphsCount}
                                    - {(book.translatedParagraphsCount / book.paragraphsCount * 100).toFixed(0)}% translated
                                {/if}
                            </a>
                            <button onclick="{() => requestDeleteBook(book)}">Delete</button>
                        </li>
                    {/each}
                </ul>
            {/if}
            
            <!-- Subfolders -->
            {#if folder.folders.length > 0}
                {#each folder.folders as subfolder}
                    {@render FolderComponent(subfolder)}
                {/each}
            {/if}
        </div>
    </details>
{/snippet}

<style>
    /* Hide the chevron for root folder (empty summary) */
    summary:empty {
        list-style: none;
    }
    
    summary:empty::-webkit-details-marker {
        display: none;
    }
</style>
