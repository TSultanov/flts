<script lang="ts">
    import type { LibraryFolder } from "./data/library";
    import BaseDialog from "./BaseDialog.svelte";
    import CreateFolderDialog from "./CreateFolderDialog.svelte";

    let {
        isOpen = $bindable(false),
        rootFolder,
        onConfirm,
        onCancel,
    }: {
        isOpen: boolean;
        rootFolder: LibraryFolder;
        onConfirm: (newPath: string[]) => void;
        onCancel?: () => void;
    } = $props();

    let localRootFolder = $state<LibraryFolder>();

    let selectedPath: string[] = $state([]);
    let createFolderDialogOpen = $state(false);
    let pendingParentPath: string[] = $state([]);

    // Initialize local copy of root folder when dialog opens
    $effect(() => {
        if (isOpen) {
            localRootFolder = structuredClone(rootFolder);
            selectedPath = [];
        }
    });

    function handleConfirm() {
        onConfirm($state.snapshot(selectedPath));
        isOpen = false;
    }

    function handleCancel() {
        onCancel?.();
        isOpen = false;
    }

    function selectFolder(path: string[]) {
        selectedPath = path;
    }

    function isSelected(path: string[] | null): boolean {
        if (selectedPath === null && path === null) {
            return true;
        }
        if (selectedPath === null || path === null) {
            return false;
        }
        if (selectedPath.length !== path.length) {
            return false;
        }
        return selectedPath.every((segment, index) => segment === path[index]);
    }

    function getPathArray(folder: LibraryFolder, currentPath: string[] = []): string[] {
        return folder.name ? [...currentPath, folder.name] : currentPath;
    }

    function createNewFolder(parentPath: string[]) {
        pendingParentPath = parentPath;
        createFolderDialogOpen = true;
    }

    function handleCreateFolder(folderName: string) {
        if (localRootFolder) {
            const trimmedName = folderName.trim();
            const newFolderPath = [...pendingParentPath, trimmedName];

            // Find or create the folder in the local structure
            const findOrCreateFolder = (folder: LibraryFolder, pathSegments: string[]): LibraryFolder => {
                if (pathSegments.length === 0) {
                    return folder;
                }

                const [currentSegment, ...remainingSegments] = pathSegments;
                let targetFolder = folder.folders.find(f => f.name === currentSegment);

                if (!targetFolder) {
                    targetFolder = {
                        name: currentSegment,
                        folders: [],
                        books: []
                    };
                    folder.folders.push(targetFolder);
                }

                return findOrCreateFolder(targetFolder, remainingSegments);
            };

            // Add the new folder to the local structure if it doesn't exist
            findOrCreateFolder(localRootFolder, newFolderPath);

            // Select the newly created folder
            selectedPath = newFolderPath;

            // Trigger reactivity
            localRootFolder = localRootFolder;
        }
    }
</script>

<BaseDialog
    bind:isOpen
    title="Move to Folder"
    maxWidth="500px"
    {onCancel}
    testId="move-folder-dialog"
>
    <div class="folder-tree" data-testid="folder-tree">
        {#if localRootFolder}
            {@render FolderTreeComponent(localRootFolder, [])}
        {/if}
    </div>

    <div>
        <p data-testid="move-to-preview">
            <strong>Move to:</strong> /{selectedPath?.join("/")}
        </p>
    </div>

    <div class="dialog-buttons">
        <button
            onclick={handleCancel}
            class="secondary"
            data-testid="move-folder-cancel">Cancel</button
        >
        <button
            onclick={handleConfirm}
            disabled={!selectedPath}
            data-testid="move-folder-confirm">Confirm</button
        >
    </div>
</BaseDialog>

<CreateFolderDialog
    bind:isOpen={createFolderDialogOpen}
    onConfirm={handleCreateFolder}
/>

<!-- Recursive folder tree component snippet -->
{#snippet FolderTreeComponent(folder: LibraryFolder, currentPath: string[])}
    <div class="folder-option" class:selected={isSelected(currentPath)}>
        <button
            class="folder-button"
            data-testid="folder-button"
            data-folder-path={currentPath.join("/")}
            onclick={() => selectFolder(currentPath)}
        >
            {folder.name ?? "/"}
        </button>
    </div>

    <div class="nested-folder">
        {#if folder.folders.length > 0}
            {#each folder.folders as subfolder}
                {@const folderPath = getPathArray(subfolder, currentPath)}
                {@render FolderTreeComponent(subfolder, folderPath)}
            {/each}
        {/if}

        <div class="folder-option">
            <button
                class="secondary"
                data-testid="create-new-folder-button"
                onclick={() => createNewFolder(currentPath)}
                >Create new folder</button
            >
        </div>
    </div>
{/snippet}

<style>
    .folder-tree {
        max-height: 300px;
        overflow-y: auto;
        border: 1px solid var(--background-color);
        border-radius: 4px;
        padding: 8px;
        margin-bottom: 24px;
    }

    .folder-option {
        margin-bottom: 4px;
    }

    .folder-option.selected {
        background: var(--accent-color);
        border-radius: 4px;
    }

    .folder-button {
        width: 100%;
        text-align: left;
        background: none;
        border: none;
        padding: 12px 8px;
        cursor: pointer;
        color: var(--text-color);
        font-family: inherit;
        font-size: 14px;
        border-radius: 4px;
    }

    .folder-button:hover {
        background: var(--button-cancel-hover);
    }

    .folder-option.selected .folder-button {
        color: var(--background-color);
        background: var(--button-cancel-hover);
    }

    .nested-folder {
        padding-left: 5px;
        margin-left: 8px;
        border-left: 1px dotted var(--background-color);
    }
</style>
