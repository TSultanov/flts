<script lang="ts">
    import { getContext } from "svelte";
    import { Library, type LibraryFolder, type LibraryBook } from "./library.svelte";
    import { route } from "@mateothegreat/svelte5-router";
    import ConfirmDialog from "./ConfirmDialog.svelte";
    import MoveFolderDialog from "./MoveFolderDialog.svelte";

    const library: Library = getContext("library");
    const rootFolder = library.getLibraryBooks();

    // Batch selection state
    let selectedBookIds = $state(new Set<number>());
    let showBatchDeleteDialog = $state(false);
    let showBatchMoveDialog = $state(false);
    let booksToDelete: LibraryBook[] = $state([]);
    let booksToMove: LibraryBook[] = $state([]);

    // Batch selection functions
    function toggleBookSelection(bookId: number) {
        if (selectedBookIds.has(bookId)) {
            selectedBookIds.delete(bookId);
        } else {
            selectedBookIds.add(bookId);
        }
        selectedBookIds = new Set(selectedBookIds); // Trigger reactivity
    }

    function selectAllBooks() {
        if (!$rootFolder) return;
        const allBookIds = getAllBookIds($rootFolder);
        selectedBookIds = new Set(allBookIds);
    }

    function clearSelection() {
        selectedBookIds.clear();
        selectedBookIds = new Set(selectedBookIds); // Trigger reactivity
    }

    function getAllBookIds(folder: LibraryFolder): number[] {
        const bookIds: number[] = [];
        
        // Add books from current folder
        bookIds.push(...folder.books.map(book => book.id));
        
        // Recursively add books from subfolders
        for (const subfolder of folder.folders) {
            bookIds.push(...getAllBookIds(subfolder));
        }
        
        return bookIds;
    }

    function requestBatchDelete() {
        if (!$rootFolder) return;
        booksToDelete = getSelectedBooks($rootFolder);
        showBatchDeleteDialog = true;
    }

    function requestBatchMove() {
        if (!$rootFolder) return;
        booksToMove = getSelectedBooks($rootFolder);
        showBatchMoveDialog = true;
    }

    function getSelectedBooks(folder: LibraryFolder): LibraryBook[] {
        const books: LibraryBook[] = [];
        
        // Add selected books from current folder
        books.push(...folder.books.filter(book => selectedBookIds.has(book.id)));
        
        // Recursively add selected books from subfolders
        for (const subfolder of folder.folders) {
            books.push(...getSelectedBooks(subfolder));
        }
        
        return books;
    }

    function confirmBatchDelete() {
        if (booksToDelete.length > 0) {
            library.deleteBooksInBatch(booksToDelete.map(book => book.id));
            booksToDelete = [];
            clearSelection();
        }
        showBatchDeleteDialog = false;
    }

    function cancelBatchDelete() {
        booksToDelete = [];
        showBatchDeleteDialog = false;
    }

    function confirmBatchMove(newPath: string[] | null) {
        if (booksToMove.length > 0) {
            library.moveBooksInBatch(booksToMove.map(book => book.id), newPath);
            booksToMove = [];
            clearSelection();
        }
        showBatchMoveDialog = false;
    }

    function cancelBatchMove() {
        booksToMove = [];
        showBatchMoveDialog = false;
    }

    const selectedCount = $derived(selectedBookIds.size);
    const hasSelection = $derived(selectedCount > 0);
</script>

{#if $rootFolder}
    <div class="books">
        <div class="header">
            <h1>Books</h1>
            {#if hasSelection}
                <div class="batch-actions">
                    <span class="selection-count">{selectedCount} selected</span>
                    <button onclick={requestBatchMove} class="compact">Move Selected</button>
                    <button onclick={requestBatchDelete} class="danger compact">Delete Selected</button>
                    <button onclick={clearSelection} class="secondary compact">Clear Selection</button>
                </div>
            {:else}
                <div class="select-actions">
                    <button onclick={selectAllBooks} class="secondary compact">Select All</button>
                </div>
            {/if}
        </div>
        <div class="folders-container">
            {@render FolderComponent($rootFolder)}
        </div>
    </div>
{/if}

<MoveFolderDialog 
    bind:isOpen={showBatchMoveDialog}
    rootFolder={$rootFolder || { name: undefined, folders: [], books: [] }}
    onConfirm={confirmBatchMove}
    onCancel={cancelBatchMove}
/>

<!-- Batch delete confirmation dialog -->
<ConfirmDialog 
    bind:isOpen={showBatchDeleteDialog}
    title="Delete Books"
    message={booksToDelete.length > 0 ? `Are you sure you want to delete ${booksToDelete.length} book(s)? This action cannot be undone.` : ""}
    onConfirm={confirmBatchDelete}
    onCancel={cancelBatchDelete}
/>

<!-- Recursive folder component snippet -->
{#snippet FolderComponent(folder: LibraryFolder)}
    <details open={!folder.name}>
        {#if folder.name}
            <summary>{folder.name}</summary>
        {:else}
            <summary></summary>
        {/if}
        <!-- Subfolders -->
        <div class="subfolders">
            {#if folder.folders.length > 0}
                {#each folder.folders as subfolder}
                    {@render FolderComponent(subfolder)}
                {/each}
            {/if}
        </div>
        <div class="subfolder-books">        
            <!-- Books in this folder -->
            {#if folder.books.length > 0}
                <ul>
                    {#each folder.books as book}
                        <li>
                            <div class="book-selection">
                                <label class="book-checkbox">
                                    <input 
                                        type="checkbox" 
                                        checked={selectedBookIds.has(book.id)}
                                        onchange={() => toggleBookSelection(book.id)}
                                    />
                                </label>
                                <a use:route href="/book/{book.id}">{book.title} - {book.chapters.length} chapter(s)
                                    {#if book.translatedParagraphsCount != book.paragraphsCount}
                                        - {(book.translatedParagraphsCount / book.paragraphsCount * 100).toFixed(0)}% translated
                                    {/if}
                                </a>
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
        padding: 0 10px;
        display: flex;
        flex-direction: column;
    }

    .folders-container {
        overflow-y: auto;
    }

    .header {
        display: flex;
        align-items: center;
        justify-content: space-between;
        padding: 10px 0;
        border-bottom: 1px solid var(--background-color);
    }

    .header h1 {
        margin: 0;
    }

    .batch-actions, .select-actions {
        display: flex;
        align-items: center;
        gap: 8px;
    }

    .selection-count {
        color: var(--text-inactive);
        font-size: 14px;
        margin-right: 8px;
    }

    .book-selection {
        display: flex;
        align-items: center;
        gap: 8px;
        flex: 1;
    }

    .book-checkbox {
        display: flex;
        align-items: center;
        cursor: pointer;
    }

    .book-checkbox input[type="checkbox"] {
        margin: 0;
        cursor: pointer;
    }

    li {
        display: flex;
        align-items: center;
        margin-bottom: 1px;
        padding: 8px 0 8px 10px;
        border-bottom: 1px solid var(--background-color);
    }

    li a {
        flex: 1;
        text-decoration: none;
        color: inherit;
    }

    li a:hover {
        text-decoration: underline;
    }

    ul {
        padding: 0;
        margin: 0;
    }

    .subfolders {
        margin: 10px;
    }

    .subfolder-books {
        border-left: 1px solid var(--background-color);
        border-top: 1px solid var(--background-color);
        border-right: 1px solid var(--background-color);
        margin: 10px;
    }
</style>
