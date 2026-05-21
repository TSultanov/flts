<script lang="ts">
    import type { Snippet } from "svelte";

    type Side = "left" | "right" | "top" | "bottom";

    function clamp(value: number, min: number, max: number): number {
        return Math.min(Math.max(value, min), max);
    }

    function isTypingTarget(target: EventTarget | null): boolean {
        if (!(target instanceof HTMLElement)) return false;
        if (target.isContentEditable) return true;
        const tag = target.tagName;
        return tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT";
    }

    let {
        side,
        expanded = $bindable(false),
        size = $bindable(0),
        minSize,
        maxSizeRatio = 0.5,
        maxSize,
        collapsedSize = 0,
        shortcut,
        testId,
        children,
        peek,
    }: {
        side: Side;
        expanded?: boolean;
        size?: number;
        minSize: number;
        maxSizeRatio?: number;
        maxSize?: number;
        collapsedSize?: number;
        shortcut?: string;
        testId?: string;
        children: Snippet;
        peek?: Snippet;
    } = $props();

    const horizontal = $derived(side === "left" || side === "right");
    const hasPeek = $derived(peek != null && collapsedSize > 0);

    let panelEl = $state<HTMLElement | null>(null);
    let isResizing = $state(false);
    let dragStartCoord = 0;
    let dragStartSize = 0;

    function maxAllowedSize(): number {
        // In slot mode the panel-host itself is sized at `collapsedSize`;
        // for the max bound we want the container the user *would* be
        // resizing into — the chapter-view (panelEl's parent).
        const parent = panelEl?.parentElement;
        const dim = parent
            ? horizontal
                ? parent.clientWidth
                : parent.clientHeight
            : Number.POSITIVE_INFINITY;
        const ratioBound = dim * maxSizeRatio;
        return maxSize != null ? Math.min(maxSize, ratioBound) : ratioBound;
    }

    function toggle() {
        expanded = !expanded;
    }

    function onResizePointerDown(event: PointerEvent) {
        if (event.button !== 0) return;
        event.preventDefault();
        (event.currentTarget as Element).setPointerCapture(event.pointerId);
        dragStartCoord = horizontal ? event.clientX : event.clientY;
        dragStartSize = size;
        isResizing = true;
    }

    function onResizePointerMove(event: PointerEvent) {
        const target = event.currentTarget as Element;
        if (!target.hasPointerCapture(event.pointerId)) return;
        const coord = horizontal ? event.clientX : event.clientY;
        const rawDelta = coord - dragStartCoord;
        // For panels anchored to right / bottom, dragging "into" the
        // viewport (smaller coord) grows the panel — sign flips.
        const directionSign =
            side === "left" || side === "top" ? 1 : -1;
        size = clamp(
            dragStartSize + rawDelta * directionSign,
            minSize,
            maxAllowedSize(),
        );
    }

    function onResizePointerUp(event: PointerEvent) {
        const target = event.currentTarget as Element;
        if (!target.hasPointerCapture(event.pointerId)) return;
        target.releasePointerCapture(event.pointerId);
        isResizing = false;
    }

    function handleKeydown(event: KeyboardEvent) {
        if (!shortcut) return;
        if (event.key.toLowerCase() !== shortcut.toLowerCase()) return;
        if (event.ctrlKey || event.metaKey || event.altKey) return;
        if (isTypingTarget(event.target)) return;
        event.preventDefault();
        toggle();
    }

    // In overlay mode the panel always takes its `size` (transform hides
    // it when collapsed). In slot mode the host reserves `collapsedSize`
    // and the body overflows up to `size` when expanded.
    const overlaySize = $derived(size);
    const showsContent = $derived(expanded || hasPeek);
</script>

<svelte:window onkeydown={handleKeydown} />

{#if hasPeek}
    <!-- Slot mode: host is in flex flow (takes `collapsedSize`), inner
         body absolute-overflows up to `size` when expanded so opening
         the panel doesn't resize the chapter viewport. The visible
         panel UI is the body — that's where `testId` goes. -->
    <aside
        class="panel-host slot side-{side}"
        class:expanded
        class:resizing={isResizing}
        style="--collapsed-size: {collapsedSize}px; --panel-size: {size}px"
        bind:this={panelEl}
    >
        <div class="panel-body" data-testid={testId}>
            {#if !expanded}
                {@render peek!()}
            {:else}
                {@render children()}
            {/if}
            {#if expanded}
                <div
                    class="resize-grip"
                    role="separator"
                    aria-orientation={horizontal ? "vertical" : "horizontal"}
                    aria-label="Resize panel"
                    data-testid={testId ? `${testId}-resize` : undefined}
                    onpointerdown={onResizePointerDown}
                    onpointermove={onResizePointerMove}
                    onpointerup={onResizePointerUp}
                    onpointercancel={onResizePointerUp}
                ></div>
            {/if}
        </div>
    </aside>
{:else}
    <!-- Overlay mode: panel is absolute-positioned and slides in/out
         entirely via transform. Children always render so DOM is stable
         for selectors. -->
    <aside
        class="panel-host overlay side-{side}"
        class:expanded
        class:resizing={isResizing}
        class:hidden={!showsContent}
        style="--panel-size: {overlaySize}px"
        aria-hidden={!showsContent}
        data-testid={testId}
        bind:this={panelEl}
    >
        {@render children()}
        {#if expanded}
            <div
                class="resize-grip"
                role="separator"
                aria-orientation={horizontal ? "vertical" : "horizontal"}
                aria-label="Resize panel"
                data-testid={testId ? `${testId}-resize` : undefined}
                onpointerdown={onResizePointerDown}
                onpointermove={onResizePointerMove}
                onpointerup={onResizePointerUp}
                onpointercancel={onResizePointerUp}
            ></div>
        {/if}
    </aside>
{/if}

<style>
    .panel-host {
        display: flex;
        background-color: var(--dialog-background);
    }

    /* --- Overlay mode (chapters) --- */

    .panel-host.overlay {
        position: absolute;
        z-index: 10;
        box-shadow: 0 0 6px rgba(0, 0, 0, 0.08);
        transition:
            width 180ms ease,
            height 180ms ease,
            transform 180ms ease;
    }

    .panel-host.overlay.resizing {
        transition: none;
    }

    .panel-host.overlay.side-left {
        top: 0;
        left: 0;
        bottom: 0;
        width: var(--panel-size);
        flex-direction: column;
        border-right: 1px solid var(--background-color);
        transform: translateX(-100%);
    }
    .panel-host.overlay.side-left.expanded,
    .panel-host.overlay.side-left:not(.hidden) {
        transform: translateX(0);
    }

    .panel-host.overlay.side-right {
        top: 0;
        right: 0;
        bottom: 0;
        width: var(--panel-size);
        flex-direction: column;
        border-left: 1px solid var(--background-color);
        transform: translateX(100%);
    }
    .panel-host.overlay.side-right.expanded,
    .panel-host.overlay.side-right:not(.hidden) {
        transform: translateX(0);
    }

    .panel-host.overlay.side-top {
        top: 0;
        left: 0;
        right: 0;
        height: var(--panel-size);
        flex-direction: column;
        border-bottom: 1px solid var(--background-color);
        transform: translateY(-100%);
    }
    .panel-host.overlay.side-top.expanded,
    .panel-host.overlay.side-top:not(.hidden) {
        transform: translateY(0);
    }

    .panel-host.overlay.side-bottom {
        bottom: 0;
        left: 0;
        right: 0;
        height: var(--panel-size);
        flex-direction: column;
        border-top: 1px solid var(--background-color);
        transform: translateY(100%);
    }
    .panel-host.overlay.side-bottom.expanded,
    .panel-host.overlay.side-bottom:not(.hidden) {
        transform: translateY(0);
    }

    /* Overlay-mode resize-grip lives directly inside the panel-host. */
    .panel-host.overlay > .resize-grip {
        position: absolute;
        touch-action: none;
    }
    .panel-host.overlay.side-left > .resize-grip {
        top: 0;
        right: 0;
        width: 6px;
        height: 100%;
        cursor: ew-resize;
    }
    .panel-host.overlay.side-right > .resize-grip {
        top: 0;
        left: 0;
        width: 6px;
        height: 100%;
        cursor: ew-resize;
    }
    .panel-host.overlay.side-top > .resize-grip {
        left: 0;
        bottom: 0;
        height: 6px;
        width: 100%;
        cursor: ns-resize;
    }
    .panel-host.overlay.side-bottom > .resize-grip {
        left: 0;
        top: 0;
        height: 6px;
        width: 100%;
        cursor: ns-resize;
    }

    /* --- Slot mode (word view) --- */

    .panel-host.slot {
        position: relative;
        flex: 0 0 auto;
        /* Establish a stacking context above the overlay panels (z:10) so
           the expanded body, which overflows upward into the chapter
           viewport, paints on top of the chapters panel where they meet. */
        z-index: 20;
    }
    .panel-host.slot.side-top,
    .panel-host.slot.side-bottom {
        height: var(--collapsed-size);
        width: 100%;
    }
    .panel-host.slot.side-left,
    .panel-host.slot.side-right {
        width: var(--collapsed-size);
        height: 100%;
    }

    .panel-host.slot .panel-body {
        position: absolute;
        background-color: var(--dialog-background);
        box-shadow: 0 0 6px rgba(0, 0, 0, 0.08);
        display: flex;
        transition:
            width 180ms ease,
            height 180ms ease;
    }
    .panel-host.slot.resizing .panel-body {
        transition: none;
    }

    .panel-host.slot.side-bottom .panel-body {
        left: 0;
        right: 0;
        bottom: 0;
        height: var(--collapsed-size);
        flex-direction: column;
        border-top: 1px solid var(--background-color);
    }
    .panel-host.slot.side-bottom.expanded .panel-body {
        height: var(--panel-size);
    }

    .panel-host.slot.side-top .panel-body {
        left: 0;
        right: 0;
        top: 0;
        height: var(--collapsed-size);
        flex-direction: column;
        border-bottom: 1px solid var(--background-color);
    }
    .panel-host.slot.side-top.expanded .panel-body {
        height: var(--panel-size);
    }

    .panel-host.slot.side-left .panel-body {
        top: 0;
        bottom: 0;
        left: 0;
        width: var(--collapsed-size);
        flex-direction: column;
        border-right: 1px solid var(--background-color);
    }
    .panel-host.slot.side-left.expanded .panel-body {
        width: var(--panel-size);
    }

    .panel-host.slot.side-right .panel-body {
        top: 0;
        bottom: 0;
        right: 0;
        width: var(--collapsed-size);
        flex-direction: column;
        border-left: 1px solid var(--background-color);
    }
    .panel-host.slot.side-right.expanded .panel-body {
        width: var(--panel-size);
    }

    .panel-host.slot .resize-grip {
        position: absolute;
        touch-action: none;
    }
    .panel-host.slot.side-bottom .resize-grip {
        left: 0;
        top: 0;
        height: 6px;
        width: 100%;
        cursor: ns-resize;
    }
    .panel-host.slot.side-top .resize-grip {
        left: 0;
        bottom: 0;
        height: 6px;
        width: 100%;
        cursor: ns-resize;
    }
    .panel-host.slot.side-left .resize-grip {
        top: 0;
        right: 0;
        width: 6px;
        height: 100%;
        cursor: ew-resize;
    }
    .panel-host.slot.side-right .resize-grip {
        top: 0;
        left: 0;
        width: 6px;
        height: 100%;
        cursor: ew-resize;
    }

    .resize-grip:hover,
    .resize-grip:active {
        background-color: var(--accent-color);
    }
</style>
