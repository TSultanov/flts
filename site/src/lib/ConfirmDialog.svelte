<script lang="ts">
    import BaseDialog from "./BaseDialog.svelte";

    let {
        isOpen = $bindable(false),
        title = "Confirm Action",
        message = "Are you sure you want to proceed?",
        onConfirm,
        onCancel,
    }: {
        isOpen: boolean;
        title?: string;
        message?: string;
        onConfirm: () => void;
        onCancel?: () => void;
    } = $props();

    function handleConfirm() {
        onConfirm();
        isOpen = false;
    }

    function handleCancel() {
        onCancel?.();
        isOpen = false;
    }
</script>

<BaseDialog bind:isOpen {title} {onCancel} testId="confirm-dialog">
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
</BaseDialog>

<style>
    p {
        margin: 0 0 24px 0;
        color: var(--dialog-text-secondary);
    }
</style>
