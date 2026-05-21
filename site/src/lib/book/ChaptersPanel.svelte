<script lang="ts">
    import Fa from "svelte-fa";
    import { faChevronLeft, faChevronRight } from "@fortawesome/free-solid-svg-icons";
    import type { ChapterMetaView } from "../data/library";
    import type { UUID } from "../data/uuid";

    const {
        bookId,
        chapters,
        currentChapterId,
    }: {
        bookId: UUID;
        chapters: ChapterMetaView[];
        currentChapterId: number | null;
    } = $props();

    const OPEN_KEY = "flts.bookview.chaptersPanel.open";
    const WIDTH_KEY = "flts.bookview.chaptersPanel.width";
    const DEFAULT_WIDTH = 260;
    const MIN_WIDTH = 150;
    const MAX_WIDTH_RATIO = 0.5;

    function clamp(value: number, min: number, max: number): number {
        return Math.min(Math.max(value, min), max);
    }

    function readInitialOpen(): boolean {
        try {
            return localStorage.getItem(OPEN_KEY) === "true";
        } catch {
            return false;
        }
    }

    function readInitialWidth(): number {
        try {
            const raw = localStorage.getItem(WIDTH_KEY);
            if (raw == null) return DEFAULT_WIDTH;
            const parsed = parseFloat(raw);
            if (!Number.isFinite(parsed)) return DEFAULT_WIDTH;
            return Math.max(parsed, MIN_WIDTH);
        } catch {
            return DEFAULT_WIDTH;
        }
    }

    let isOpen = $state(readInitialOpen());
    let width = $state(readInitialWidth());
    let panelEl = $state<HTMLElement | null>(null);

    function persistOpen(value: boolean) {
        try {
            localStorage.setItem(OPEN_KEY, value ? "true" : "false");
        } catch {
            // localStorage may be unavailable; the in-memory state still works.
        }
    }

    function persistWidth(value: number) {
        try {
            localStorage.setItem(WIDTH_KEY, value.toString());
        } catch {
            // ditto
        }
    }

    function toggle() {
        isOpen = !isOpen;
        persistOpen(isOpen);
    }

    function isTypingTarget(target: EventTarget | null): boolean {
        if (!(target instanceof HTMLElement)) return false;
        if (target.isContentEditable) return true;
        const tag = target.tagName;
        return tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT";
    }

    function handleKeydown(event: KeyboardEvent) {
        if (event.key !== "c" && event.key !== "C") return;
        if (event.ctrlKey || event.metaKey || event.altKey) return;
        if (isTypingTarget(event.target)) return;
        event.preventDefault();
        toggle();
    }

    function maxWidthForContainer(): number {
        const el = panelEl?.parentElement;
        if (!el) return Number.POSITIVE_INFINITY;
        return el.clientWidth * MAX_WIDTH_RATIO;
    }

    let dragStartX = 0;
    let dragStartWidth = 0;
    let isResizing = $state(false);

    function onResizePointerDown(event: PointerEvent) {
        if (event.button !== 0) return;
        event.preventDefault();
        (event.currentTarget as Element).setPointerCapture(event.pointerId);
        dragStartX = event.clientX;
        dragStartWidth = width;
        isResizing = true;
    }

    function onResizePointerMove(event: PointerEvent) {
        const target = event.currentTarget as Element;
        if (!target.hasPointerCapture(event.pointerId)) return;
        const delta = event.clientX - dragStartX;
        width = clamp(dragStartWidth + delta, MIN_WIDTH, maxWidthForContainer());
    }

    function onResizePointerUp(event: PointerEvent) {
        const target = event.currentTarget as Element;
        if (!target.hasPointerCapture(event.pointerId)) return;
        target.releasePointerCapture(event.pointerId);
        isResizing = false;
        persistWidth(width);
    }

    function handleChapterClick() {
        if (!isOpen) return;
        isOpen = false;
        persistOpen(false);
    }
</script>

<svelte:window onkeydown={handleKeydown} />

<aside
    class="panel"
    class:open={isOpen}
    class:resizing={isResizing}
    style="width: {width}px"
    aria-hidden={!isOpen}
    data-testid="chapters-panel"
    bind:this={panelEl}
>
    <nav class="chapters">
        {#each chapters as chapter}
            <p class={chapter.id === currentChapterId ? "current" : ""}>
                <a
                    href="/book/{bookId}/{chapter.id}"
                    onclick={handleChapterClick}
                >
                    {chapter.title ? chapter.title : "<no title>"}
                </a>
            </p>
        {/each}
    </nav>
    <div
        class="resize-grip"
        role="separator"
        aria-orientation="vertical"
        aria-label="Resize chapters panel"
        data-testid="chapters-panel-resize"
        onpointerdown={onResizePointerDown}
        onpointermove={onResizePointerMove}
        onpointerup={onResizePointerUp}
        onpointercancel={onResizePointerUp}
    ></div>
</aside>

<button
    type="button"
    class="edge-handle"
    class:open={isOpen}
    class:resizing={isResizing}
    style="--panel-width: {width}px"
    aria-label={isOpen ? "Hide chapters" : "Show chapters"}
    aria-expanded={isOpen}
    data-testid="chapters-panel-handle"
    onclick={toggle}
>
    <Fa icon={isOpen ? faChevronLeft : faChevronRight} />
</button>

<style>
    .panel {
        position: absolute;
        top: 0;
        left: 0;
        bottom: 0;
        z-index: 10;
        display: flex;
        flex-direction: column;
        background-color: var(--dialog-background);
        border-right: 1px solid var(--background-color);
        box-shadow: 2px 0 6px rgba(0, 0, 0, 0.08);
        transform: translateX(-100%);
        transition: transform 180ms ease;
        min-width: 150px;
    }

    .panel.open {
        transform: translateX(0);
    }

    /* Drop animation while dragging so the handle stays attached to the
       panel's right edge during fast resizes. */
    .panel.resizing,
    .edge-handle.resizing {
        transition: none;
    }

    .chapters {
        flex: 1 1 auto;
        padding: 10px;
        overflow-y: auto;
        overflow-x: hidden;
    }

    .chapters p {
        margin: 0.25em 0;
    }

    .chapters .current {
        outline: 1px dotted var(--selected-color);
    }

    .resize-grip {
        position: absolute;
        top: 0;
        right: 0;
        width: 6px;
        height: 100%;
        cursor: ew-resize;
        touch-action: none;
    }

    .resize-grip:hover,
    .resize-grip:active {
        background-color: var(--accent-color);
    }

    .edge-handle {
        position: absolute;
        top: 50%;
        left: 0;
        z-index: 11;
        /* Centering via negative margin so the global `button:active`
           transform from app.css doesn't override our centering and yank
           the button out of position mid-click. */
        margin-top: -28px;
        width: 22px;
        height: 56px;
        padding: 0;
        border: 1px solid var(--background-color);
        border-left: none;
        border-radius: 0 6px 6px 0;
        background-color: var(--dialog-background);
        color: var(--dialog-text);
        cursor: pointer;
        display: flex;
        align-items: center;
        justify-content: center;
        box-shadow: 2px 0 4px rgba(0, 0, 0, 0.15);
        transition: left 180ms ease;
    }

    .edge-handle:hover:not(:disabled) {
        background-color: var(--button-cancel-hover);
    }

    .edge-handle :global(svg) {
        pointer-events: none;
    }

    .edge-handle.open {
        left: var(--panel-width);
    }
</style>
