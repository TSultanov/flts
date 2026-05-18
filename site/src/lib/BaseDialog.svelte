<script lang="ts">
    import type { Snippet } from "svelte";

    let {
        isOpen = $bindable(false),
        title,
        maxWidth = "400px",
        onCancel,
        testId,
        children,
    }: {
        isOpen: boolean;
        title: string;
        maxWidth?: string;
        onCancel?: () => void;
        testId?: string;
        children: Snippet;
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
</script>

<dialog
    bind:this={dialog}
    oncancel={() => onCancel?.()}
    onclose={() => (isOpen = false)}
    data-testid={testId}
    style="max-width: {maxWidth}"
>
    <div class="dialog-content">
        <h3>{title}</h3>
        {@render children()}
    </div>
</dialog>

<style>
    dialog {
        border: 1px solid var(--dialog-border);
        border-radius: 8px;
        padding: 0;
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

    :global(.dialog-buttons) {
        display: flex;
        gap: 12px;
        justify-content: flex-end;
    }
</style>
