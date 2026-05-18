<script lang="ts">
    import { fade } from "svelte/transition";
    import { sizeOverlay } from "./translationOverlay";

    let {
        text,
        sentence,
        word,
        flatIndex,
        translation,
        visible,
        selected,
        onClick,
    }: {
        text: string;
        sentence: number;
        word: number;
        flatIndex: number;
        translation: string | null;
        visible: boolean;
        selected: boolean;
        onClick: (info: { sentence: number; word: number; flatIndex: number }) => void;
    } = $props();

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
    bind:this={spanEl}
    onclick={() => onClick({ sentence, word, flatIndex })}
>{#if (visible || selected) && translation}<span
        class="translation-overlay"
        aria-hidden="true"
        bind:this={overlayEl}
        transition:fade={{ duration: 150 }}
    >{translation}</span>{/if}{text}</span>

<style>
    .word-span {
        position: relative;
        display: inline-block;
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
