<script lang="ts">
    let { 
        isOpen = $bindable(false),
        onConfirm,
        onCancel
    }: {
        isOpen: boolean,
        onConfirm: (folderName: string) => void,
        onCancel?: () => void
    } = $props();

    let dialog: HTMLDialogElement;
    let folderName = $state("");
    let inputElement: HTMLInputElement;

    $effect(() => {
        if (dialog) {
            if (isOpen) {
                dialog.showModal();
                folderName = "";
                // Focus the input after dialog opens
                setTimeout(() => {
                    inputElement?.focus();
                }, 0);
            } else {
                dialog.close();
            }
        }
    });

    function handleConfirm() {
        const trimmedName = folderName.trim();
        if (trimmedName) {
            onConfirm(trimmedName);
            isOpen = false;
        }
    }

    function handleFormSubmit(event: Event) {
        event.preventDefault();
        handleConfirm();
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

    function handleKeydown(event: KeyboardEvent) {
        if (event.key === "Enter") {
            event.preventDefault();
            handleConfirm();
        } else if (event.key === "Escape") {
            event.preventDefault();
            handleCancel();
        }
    }

    const isValidName = $derived(folderName.trim().length > 0);
</script>

<dialog bind:this={dialog} onclose={handleDialogClose}>
    <div class="dialog-content">
        <h3>Create New Folder</h3>
        
        <form onsubmit={handleFormSubmit}>
            <div class="input-group">
                <label for="folder-name">Folder Name:</label>
                <input
                    id="folder-name"
                    type="text"
                    bind:this={inputElement}
                    bind:value={folderName}
                    onkeydown={handleKeydown}
                    placeholder="Enter folder name"
                    autocomplete="off"
                />
            </div>
            
            <div class="dialog-buttons">
                <button type="button" onclick={handleCancel} class="secondary">Cancel</button>
                <button type="submit" disabled={!isValidName}>Create</button>
            </div>
        </form>
    </div>
</dialog>

<style>
    dialog {
        border: 1px solid var(--dialog-border);
        border-radius: 8px;
        padding: 0;
        max-width: 400px;
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

    .input-group {
        margin-bottom: 24px;
    }

    .input-group label {
        display: block;
        margin-bottom: 8px;
        font-weight: 500;
        color: var(--text-color);
    }

    .input-group input {
        width: 100%;
        padding: 12px;
        border: 1px solid var(--background-color);
        border-radius: 4px;
        font-size: 14px;
        font-family: inherit;
        color: var(--text-color);
        box-sizing: border-box;
    }

    .input-group input:focus {
        outline: none;
        border-color: var(--accent-color);
        box-shadow: 0 0 0 2px var(--accent-color-transparent);
    }

    .input-group input::placeholder {
        color: var(--text-color-muted);
    }

    .dialog-buttons {
        display: flex;
        gap: 12px;
        justify-content: flex-end;
    }

    form {
        margin: 0;
    }
</style>
