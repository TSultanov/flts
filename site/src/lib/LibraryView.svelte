<script lang="ts">
    import { getContext } from "svelte";
    import { Library, type LibraryFolder, type LibraryBook } from "./library.svelte";
    import { route } from "@mateothegreat/svelte5-router";
    import ConfirmDialog from "./ConfirmDialog.svelte";
    import MoveFolderDialog from "./MoveFolderDialog.svelte";

    const library: Library = getContext("library");
    const rootFolder = library.getLibraryBooks();

    let showDeleteDialog = $state(false);
    let bookToDelete: LibraryBook | null = $state(null);

    let showMoveDialog = $state(false);
    let bookToMove: LibraryBook | null = $state(null);

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

    function requestMoveBook(book: LibraryBook) {
        bookToMove = book;
        showMoveDialog = true;
    }

    function confirmMoveBook(newPath: string[] | null) {
        if (bookToMove) {
            library.moveBook(bookToMove.id, newPath);
            bookToMove = null;
        }
        showMoveDialog = false;
    }

    function cancelMoveBook() {
        bookToMove = null;
        showMoveDialog = false;
    }
</script>

{#if $rootFolder}
    <div class="books">
        <h1>Books</h1>
        {@render FolderComponent($rootFolder)}
    </div>
{/if}

<ConfirmDialog 
    bind:isOpen={showDeleteDialog}
    title="Delete Book"
    message={bookToDelete ? `Are you sure you want to delete "${bookToDelete.title}"? This action cannot be undone.` : ""}
    onConfirm={confirmDeleteBook}
    onCancel={cancelDeleteBook}
/>

<MoveFolderDialog 
    bind:isOpen={showMoveDialog}
    rootFolder={$rootFolder || { name: undefined, folders: [], books: [] }}
    onConfirm={confirmMoveBook}
    onCancel={cancelMoveBook}
/>

<!-- Recursive folder component snippet -->
{#snippet FolderComponent(folder: LibraryFolder)}
    <details open>
        {#if folder.name}
            <summary>{folder.name}</summary>
        {:else}
            <summary></summary>
        {/if}
        <div class="{folder.name ? "subfolders" : ""}">        
            <!-- Subfolders -->
            {#if folder.folders.length > 0}
                {#each folder.folders as subfolder}
                    {@render FolderComponent(subfolder)}
                {/each}
            {/if}

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
                            <div class="book-actions">
                                <button onclick="{() => requestMoveBook(book)}" class="compact">Move</button>
                                <button onclick="{() => requestDeleteBook(book)}" class="danger compact">Delete</button>
                            </div>
                        </li>
                    {/each}
                </ul>
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

    .books {
        height: 100%;
        overflow-y: auto;
        padding: 0 10px;
    }

    .book-actions {
        display: flex;
        gap: 8px;
        margin-left: 12px;
    }

    li {
        display: flex;
        align-items: center;
        justify-content: space-between;
        margin-bottom: 1px;
        padding-bottom: 1px;
        border-bottom: 1px solid var(--background-color);
        padding-left: 10px;
    }

    li:last-child {
        border-bottom: none;
    }

    li a {
        flex: 1;
    }

    ul {
        padding: 0;
    }

    .subfolders {
        border-left: 1px dotted var(--background-color);
        margin-left: 10px;
    }
</style>
