<script lang="ts">
    import { getContext } from "svelte";
    import { Library, type IBookMeta, type LibraryFolder } from "./data/library.svelte";
    import { route } from "@mateothegreat/svelte5-router";
    import ConfirmDialog from "./ConfirmDialog.svelte";
    import MoveFolderDialog from "./MoveFolderDialog.svelte";
    import type { BookId, DatabaseSchema } from "./data/evolu/schema";
    import type { Books } from "./data/evolu/book";
    import type { Evolu } from "@evolu/common";
    import { queryState } from "@evolu/svelte";

    const library: Library = getContext("library");
    const evolu: Evolu<DatabaseSchema> = getContext("evolu");
    const books: Books = getContext("books");

    const allBooks = queryState(evolu, () => books.allBooks);
        
    const rootFolder = $derived.by(() => {
        const root: LibraryFolder = {
            folders: [],
            books: []
        };

        const getOrCreateFolder = (path: string[]): LibraryFolder => {
            if (path.length === 0) {
                return root;
            }

            let current = root;
            for (const folderName of path) {
                let folder = current.folders.find(f => f.name === folderName);
                if (!folder) {
                    folder = {
                        name: folderName,
                        folders: [],
                        books: []
                    };
                    current.folders.push(folder);
                }
                current = folder;
            }
            return current;
        };

        for (const book of allBooks.rows) {
            const path: string[] = JSON.parse(book?.path ?? "[]")
            const targetFolder = getOrCreateFolder(path);
            targetFolder.books.push({
                id: book.id,
                title: book.title ?? "<unknown>",
                path,
                chapterCount: book.chapterCount ?? 0,
                translationRatio: (book.translatedParagraphsCount ?? 0) / (book.paragraphsCount ?? 0),
            });
        }

        return root;
    });

    // Batch selection state
    let selectedBookIds = $state(new Set<BookId>());
    let showBatchDeleteDialog = $state(false);
    let showBatchMoveDialog = $state(false);
    let booksToDelete: IBookMeta[] = $state([]);
    let booksToMove: IBookMeta[] = $state([]);

    // Batch selection functions
    function toggleBookSelection(bookId: BookId) {
        if (selectedBookIds.has(bookId)) {
            selectedBookIds.delete(bookId);
        } else {
            selectedBookIds.add(bookId);
        }
        selectedBookIds = new Set(selectedBookIds); // Trigger reactivity
    }

    function selectAllBooks() {
        if (!rootFolder) return;
        const allBookUids = getAllBookIds(rootFolder);
        selectedBookIds = new Set(allBookUids);
    }

    function clearSelection() {
        selectedBookIds.clear();
        selectedBookIds = new Set(selectedBookIds); // Trigger reactivity
    }

    function getAllBookIds(folder: LibraryFolder): BookId[] {
        const bookUids: BookId[] = [];

        // Add books from current folder
        bookUids.push(...folder.books.map((book) => book.id));

        // Recursively add books from subfolders
        for (const subfolder of folder.folders) {
            bookUids.push(...getAllBookIds(subfolder));
        }

        return bookUids;
    }

    function requestBatchDelete() {
        if (!rootFolder) return;
        booksToDelete = getSelectedBooks(rootFolder);
        showBatchDeleteDialog = true;
    }

    function requestBatchMove() {
        if (!rootFolder) return;
        booksToMove = getSelectedBooks(rootFolder);
        showBatchMoveDialog = true;
    }

    function getSelectedBooks(folder: LibraryFolder): IBookMeta[] {
        const books: IBookMeta[] = [];

        // Add selected books from current folder
        books.push(
            ...folder.books.filter((book) => selectedBookIds.has(book.id)),
        );

        // Recursively add selected books from subfolders
        for (const subfolder of folder.folders) {
            books.push(...getSelectedBooks(subfolder));
        }

        return books;
    }

    function confirmBatchDelete() {
        if (booksToDelete.length > 0) {
            library.deleteBooksInBatch(booksToDelete.map((book) => book.id));
            booksToDelete = [];
            clearSelection();
        }
        showBatchDeleteDialog = false;
    }

    function cancelBatchDelete() {
        booksToDelete = [];
        showBatchDeleteDialog = false;
    }

    function confirmBatchMove(newPath: string[]) {
        if (booksToMove.length > 0) {
            library.moveBooksInBatch(
                booksToMove.map((book) => book.id),
                newPath,
            );
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

<div class="books">
    <div class="header">
        <h1>Books</h1>
        {#if hasSelection}
            <div class="batch-actions">
                <span class="selection-count">{selectedCount} selected</span
                >
                <button onclick={requestBatchMove} class="compact"
                    >Move Selected</button
                >
                <button onclick={requestBatchDelete} class="danger compact"
                    >Delete Selected</button
                >
                <button onclick={clearSelection} class="secondary compact"
                    >Clear Selection</button
                >
            </div>
        {:else}
            <div class="select-actions">
                <button onclick={selectAllBooks} class="secondary compact"
                    >Select All</button
                >
            </div>
        {/if}
    </div>
    <div class="folders-container">
        {@render FolderComponent(rootFolder)}
    </div>
</div>

<MoveFolderDialog
    bind:isOpen={showBatchMoveDialog}
    rootFolder={rootFolder || { name: undefined, folders: [], books: [] }}
    onConfirm={confirmBatchMove}
    onCancel={cancelBatchMove}
/>

<!-- Batch delete confirmation dialog -->
<ConfirmDialog
    bind:isOpen={showBatchDeleteDialog}
    title="Delete Books"
    message={booksToDelete.length > 0
        ? `Are you sure you want to delete ${booksToDelete.length} book(s)? This action cannot be undone.`
        : ""}
    onConfirm={confirmBatchDelete}
    onCancel={cancelBatchDelete}
/>

<!-- Recursive folder component snippet -->
{#snippet FolderComponent(folder: LibraryFolder)}
    {#if folder.name}
        <details>
            <summary>{folder.name}</summary>
            {@render FolderComponentInternal(folder)}
        </details>
    {:else}
        {@render FolderComponentInternal(folder)}
    {/if}
{/snippet}

{#snippet FolderComponentInternal(folder: LibraryFolder)}
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
                                    onchange={() =>
                                        toggleBookSelection(book.id)}
                                />
                            </label>
                            <a use:route href="/book/{book.id}"
                                >{book.title} - {book.chapterCount} chapter(s)
                                {#if book.translationRatio < 1.0}
                                    - {(book.translationRatio * 100).toFixed(
                                        0,
                                    )}% translated
                                {/if}
                            </a>
                        </div>
                    </li>
                {/each}
            </ul>
        {/if}
    </div>
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
        border-bottom: 1px solid var(--text-color-muted);
    }

    .header h1 {
        margin: 0;
    }

    .batch-actions,
    .select-actions {
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
        border-left: 1px solid var(--text-color-muted);
        margin: 10px;
    }
</style>
