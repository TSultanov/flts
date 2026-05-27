<script lang="ts">
    import { fade } from "svelte/transition";
    import { sizeOverlay } from "./translationOverlay";

    let {
        text,
        sentence,
        word,
        flatIndex,
        translation,
        manualShown,
        familiarity,
        selected,
        onClick,
    }: {
        text: string;
        sentence: number;
        word: number;
        flatIndex: number;
        translation: string | null;
        manualShown: boolean;
        familiarity?: number;
        selected: boolean;
        onClick: (info: { sentence: number; word: number; flatIndex: number }) => void;
    } = $props();

    const autoShow = $derived(familiarity != null && familiarity < 0.5);
    const visible = $derived(autoShow || manualShown);

    let spanEl: HTMLSpanElement | null = $state(null);
    let overlayEl: HTMLSpanElement | null = $state(null);

    $effect(() => {
        if (!spanEl || !overlayEl || !translation) return;
        sizeOverlay(spanEl, overlayEl, text, translation);
        return () => {
            spanEl?.style.removeProperty("--word-translation-font-size");
        };
    });
</script>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<span
    class="word-span"
    class:selected
    data-flat-index={flatIndex}
    style:--familiarity-opacity={familiarity != null ? 1 - familiarity : null}
    bind:this={spanEl}
    onclick={() => onClick({ sentence, word, flatIndex })}
>{#if visible && translation}<span
        class="translation-overlay"
        aria-hidden="true"
        bind:this={overlayEl}
        transition:fade={{ duration: 150 }}
    >{translation}</span>{/if}{text}</span>

<style>
    .word-span {
        position: relative;
        display: inline-block;
        text-decoration: underline;
        text-decoration-color: rgba(214, 175, 54, var(--familiarity-opacity, 0));
        text-decoration-thickness: 2px;
        text-underline-offset: 2px;
        transition: text-decoration-color 200ms ease-out;
    }
    .word-span.selected {
        outline: 1px dotted var(--selected-color);
    }
    .translation-overlay {
        position: absolute;
        left: 0;
        right: 0;
        top: 0;
        width: 100%;
        font-size: var(--word-translation-font-size, 0.55em);
        text-align: center;
        line-height: 1;
        padding: 0.05em 0.1em;
        box-sizing: border-box;
        white-space: nowrap;
        opacity: 0.9;
        user-select: none;
        -webkit-user-select: none;
        pointer-events: none;
        z-index: 2;
        overflow: hidden;
    }
</style>
