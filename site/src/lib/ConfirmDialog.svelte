<script lang="ts">
    let { 
        isOpen = $bindable(false),
        title = "Confirm Action",
        message = "Are you sure you want to proceed?",
        onConfirm,
        onCancel
    }: {
        isOpen: boolean,
        title?: string,
        message?: string,
        onConfirm: () => void,
        onCancel?: () => void
    } = $props();

    let dialog: HTMLDialogElement;

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
        onConfirm();
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
</script>

<dialog
    bind:this={dialog}
    onclose={handleDialogClose}
    data-testid="confirm-dialog"
>
    <div class="dialog-content">
        <h3>{title}</h3>
        <p data-testid="confirm-dialog-message">{message}</p>
        <div class="dialog-buttons">
            <button
                onclick={handleCancel}
                class="secondary"
                data-testid="confirm-dialog-cancel">Cancel</button
            >
            <button
                onclick={handleConfirm}
                class="danger"
                data-testid="confirm-dialog-confirm">Confirm</button
            >
        </div>
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

    .dialog-content p {
        margin: 0 0 24px 0;
        color: var(--dialog-text-secondary);
    }

    .dialog-buttons {
        display: flex;
        gap: 12px;
        justify-content: flex-end;
    }
</style>
