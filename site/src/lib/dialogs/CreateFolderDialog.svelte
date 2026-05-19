<script lang="ts">
    import BaseDialog from "./BaseDialog.svelte";

    let {
        isOpen = $bindable(false),
        onConfirm,
        onCancel,
    }: {
        isOpen: boolean;
        onConfirm: (folderName: string) => void;
        onCancel?: () => void;
    } = $props();

    let folderName = $state("");
    let inputElement: HTMLInputElement | undefined = $state();

    $effect(() => {
        if (isOpen) {
            folderName = "";
            setTimeout(() => {
                inputElement?.focus();
            }, 0);
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
        onCancel?.();
        isOpen = false;
    }

    function handleKeydown(event: KeyboardEvent) {
        if (event.key === "Enter") {
            event.preventDefault();
            handleConfirm();
        }
    }

    const isValidName = $derived(folderName.trim().length > 0);
</script>

<BaseDialog
    bind:isOpen
    title="Create New Folder"
    {onCancel}
    testId="create-folder-dialog"
>
    <form onsubmit={handleFormSubmit}>
        <div class="input-group">
            <label for="folder-name">Folder Name:</label>
            <input
                id="folder-name"
                type="text"
                data-testid="create-folder-input"
                bind:this={inputElement}
                bind:value={folderName}
                onkeydown={handleKeydown}
                placeholder="Enter folder name"
                autocomplete="off"
            />
        </div>

        <div class="dialog-buttons">
            <button
                type="button"
                onclick={handleCancel}
                class="secondary"
                data-testid="create-folder-cancel">Cancel</button
            >
            <button
                type="submit"
                disabled={!isValidName}
                data-testid="create-folder-submit">Create</button
            >
        </div>
    </form>
</BaseDialog>

<style>
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

    form {
        margin: 0;
    }
</style>
