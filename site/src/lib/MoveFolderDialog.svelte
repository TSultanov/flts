<script lang="ts">
    import type { LibraryFolder } from "./library.svelte";

    let { 
        isOpen = $bindable(false),
        rootFolder,
        onConfirm,
        onCancel
    }: {
        isOpen: boolean,
        rootFolder: LibraryFolder,
        onConfirm: (newPath: string[] | null) => void,
        onCancel?: () => void
    } = $props();

    let dialog: HTMLDialogElement;
    let localRootFolder = $state<LibraryFolder>();

    let selectedPath: string[] | null = $state(null);

    // Initialize local copy of root folder when dialog opens
    $effect(() => {
        if (isOpen) {
            localRootFolder = structuredClone(rootFolder);
            selectedPath = null;
        }
    });

    $effect(() => {
        if (dialog) {
            if (isOpen) {
                dialog.showModal();
            } else {
                dialog.close();
            }
        }
    });

    function handleConfirm() {
        onConfirm($state.snapshot(selectedPath));
        isOpen = false;
    }

    function handleCancel() {
        if (onCancel) {
            onCancel();
        }
        isOpen = false;
    }

    function handleDialogClose() {
        isOpen = false;
    }

    function selectFolder(path: string[] | null) {
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
        const folderName = prompt("Enter folder name:");
        if (folderName && folderName.trim() && localRootFolder) {
            const trimmedName = folderName.trim();
            const newFolderPath = [...parentPath, trimmedName];
            
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

<dialog bind:this={dialog} onclose={handleDialogClose}>
    <div class="dialog-content">
        <h3>Move to Folder</h3>
        <div class="folder-tree">
            {#if localRootFolder}
                {@render FolderTreeComponent(localRootFolder, [])}
            {/if}
        </div>

        <div>
            <p><strong>Move to:</strong> /{selectedPath?.join("/")}</p>
        </div>
        
        <div class="dialog-buttons">
            <button onclick={handleCancel} class="secondary">Cancel</button>
            <button onclick={handleConfirm} disabled={!selectedPath}>Confirm</button>
        </div>
    </div>
</dialog>

<!-- Recursive folder tree component snippet -->
{#snippet FolderTreeComponent(folder: LibraryFolder, currentPath: string[])}
    <div class="folder-option" class:selected={isSelected(currentPath)}>
        <button class="folder-button" onclick={() => selectFolder(currentPath)}>
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
            <button class="secondary" onclick={() => createNewFolder(currentPath)}>Create new folder</button>
        </div>
    </div>
{/snippet}

<style>
    dialog {
        border: 1px solid var(--dialog-border);
        border-radius: 8px;
        padding: 0;
        max-width: 500px;
        width: 90%;
        background: var(--dialog-background);
    }

    dialog::backdrop {
        background-color: var(--dialog-backdrop);
    }

    .dialog-content {
        padding: 24px;
    }

    .dialog-content h3 {
        margin: 0 0 16px 0;
        font-size: 1.2em;
        color: var(--dialog-text);
    }

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

    .dialog-buttons {
        display: flex;
        gap: 12px;
        justify-content: flex-end;
    }

    .nested-folder {
        padding-left: 5px;
        margin-left: 8px;
        border-left: 1px dotted var(--background-color);
    }
</style>
