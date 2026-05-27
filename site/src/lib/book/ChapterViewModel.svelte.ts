import type { Library } from "../data/library";
import type { UUID } from "../data/uuid";
import type { WordSelection } from "./ParagraphViewModel.svelte";
import { ChapterParagraphsStore } from "./ChapterParagraphsStore.svelte";

export type ChapterVMProps = {
    bookId: UUID;
    chapterId: number;
    initialParagraphId: number | null;
    initialPageOffset: number;
    container: HTMLDivElement | null;
    onPositionChange?: (paragraphId: number, pageOffset: number) => void;
};

export type WordClickInfo = {
    paragraphId: number;
    sentence: number;
    word: number;
    flatIndex: number;
};

const SIBLING_RADIUS = 2;
const GEOM_MOUNT_THRESHOLD = 2.0;
const GEOM_UNMOUNT_THRESHOLD = 2.5;
// Safety net for the restore loop. The primary settle condition is "every
// paragraph in the chapter has reported its data is loaded"; this timeout
// only fires if some paragraph's fetch never resolves (backend stuck,
// network drop), so the user isn't held forever at a partially-correct
// scroll position.
const RESTORE_FALLBACK_MS = 3000;
// Hard cap on how long the initial-load opacity gate stays closed. If
// both the restore-path and no-restore-path reveals somehow fail to
// fire (e.g. a paragraph fetch never resolves), the user still gets a
// visible chapter rather than a permanently blank panel.
const INITIAL_REVEAL_FALLBACK_MS = 1500;
// Originals fetched ahead of the restore target so a small overshoot
// from snap correction still has paragraphs laid out. Roughly a few
// pages of small paragraphs.
const RESTORE_PREFIX_BUFFER = 10;
// Used to size the eager initial-translations window from the
// container's clientHeight. Mid-range: dialog paragraphs are ~32 px
// (1 line @ line-height 2, font 16 px), prose 128-160 px (4-5 lines),
// mixed-content average lands near 100 px.
const ESTIMATED_PARAGRAPH_HEIGHT_PX = 100;

export class ChapterViewModel {
    #library!: Library;
    #props!: ChapterVMProps;

    #store!: ChapterParagraphsStore;
    #originalsKickedFor: number | null = null;

    #paragraphIdsResource = $derived.by(() =>
        this.#library.getBookChapterParagraphIds(
            this.#props.bookId,
            this.#props.chapterId,
        ),
    );

    paragraphIds = $derived<readonly number[]>(
        this.#paragraphIdsResource.current ?? [],
    );

    // Per-paragraph mount gate. WordSpans only render for ids in this set.
    // An empty set means "not yet computed" — paragraphs render eagerly
    // until the first window measurement lands so we never flash plain text
    // on initial load. Once populated it is authoritative.
    #mountedParagraphIds: Set<number> = $state(new Set());

    #visibleParagraphId: number | null = null;
    #visiblePageOffset = 0;
    #saveTimeout: ReturnType<typeof setTimeout> | null = null;
    #lastSavedParagraph: number | null = null;
    #lastSavedPageOffset = 0;
    #isResizing = false;
    #resizeTimeout: ReturnType<typeof setTimeout> | null = null;
    #scrollRaf: number | null = null;
    #initialParagraphSyncedFor: number | null | undefined = undefined;
    #isRestoring = false;
    #readyParagraphIds = new Set<number>();
    #restoreTarget: number | null = null;
    #restorePageOffset = 0;
    #restoreFallbackTimeout: ReturnType<typeof setTimeout> | null = null;
    #anchorRaf: number | null = null;
    #restoreResizeObserver: ResizeObserver | null = null;
    #savedSnapType: string | null = null;
    #columnCount = $state(1);
    #columnCountRaf: number | null = null;
    #isInitiallyReady = $state(false);
    #initialRevealFallbackTimeout: ReturnType<typeof setTimeout> | null = null;
    #initialRevealRaf: number | null = null;
    #noRestoreRevealHook: (() => void) | null = null;

    // Ephemeral set of "user clicked to reveal" words, keyed by
    // `${paragraphId}:${flatIndex}`. Resets when the chapter is unmounted —
    // initial visibility per word is driven entirely by card familiarity.
    #revealedWordKeys: Set<string> = $state(new Set());

    constructor(library: Library, props: ChapterVMProps) {
        this.#library = library;
        this.#props = props;
        this.#store = new ChapterParagraphsStore(props.bookId, library);
        // Belt-and-braces: if neither the restore nor the no-restore
        // reveal path fires (paragraph fetch stuck, etc.), this timer
        // lifts the opacity gate so the user never sees a blank panel.
        this.#initialRevealFallbackTimeout = setTimeout(() => {
            this.#initialRevealFallbackTimeout = null;
            this.#markInitiallyReady();
        }, INITIAL_REVEAL_FALLBACK_MS);
    }

    get isInitiallyReady(): boolean {
        return this.#isInitiallyReady;
    }

    get columnCount(): number {
        return this.#columnCount;
    }

    get store(): ChapterParagraphsStore {
        return this.#store;
    }

    isMounted(paragraphId: number): boolean {
        return (
            this.#mountedParagraphIds.size === 0 ||
            this.#mountedParagraphIds.has(paragraphId)
        );
    }

    handleScroll(): void {
        if (this.#isRestoring) {
            return;
        }
        if (this.#isResizing) {
            return;
        }
        if (this.#scrollRaf !== null) {
            return;
        }
        this.#scrollRaf = requestAnimationFrame(() => {
            this.#scrollRaf = null;
            this.#updateVisibleParagraph();
            this.#recomputeColumnCount();
        });
    }

    #scheduleColumnCountRecompute(): void {
        if (this.#columnCountRaf !== null) return;
        this.#columnCountRaf = requestAnimationFrame(() => {
            this.#columnCountRaf = null;
            this.#recomputeColumnCount();
        });
    }

    #recomputeColumnCount(): void {
        const container = this.#props.container;
        if (!container) return;
        const pageWidth = container.clientWidth;
        if (pageWidth <= 0) return;
        // Math.ceil so partial columns still get a snap target at their
        // start. Math.max with 1 covers the empty-content edge case.
        const next = Math.max(1, Math.ceil(container.scrollWidth / pageWidth));
        if (next !== this.#columnCount) {
            this.#columnCount = next;
        }
    }

    handleResize(): void {
        this.#isResizing = true;
        if (this.#resizeTimeout) {
            clearTimeout(this.#resizeTimeout);
        }

        if (this.#visibleParagraphId != null) {
            this.#scrollParagraphIntoView(this.#visibleParagraphId, {
                behavior: "auto",
                block: "center",
                inline: "center",
            });
        }

        this.#resizeTimeout = setTimeout(() => {
            this.#isResizing = false;
            this.#recomputeMountWindow();
            this.#recomputeColumnCount();
        }, 200);
    }

    handleWordClick(info: WordClickInfo): WordSelection {
        // Add-only: a click reveals a hidden word and is a no-op if it's
        // already shown. New Set instance so Svelte's `$state` reactivity
        // notices the change.
        const key = `${info.paragraphId}:${info.flatIndex}`;
        if (!this.#revealedWordKeys.has(key)) {
            const next = new Set(this.#revealedWordKeys);
            next.add(key);
            this.#revealedWordKeys = next;
        }
        return {
            paragraphId: info.paragraphId,
            sentence: info.sentence,
            word: info.word,
        };
    }

    isWordRevealed(paragraphId: number, flatIndex: number): boolean {
        return this.#revealedWordKeys.has(`${paragraphId}:${flatIndex}`);
    }

    startInitialSync(): () => void {
        const ids = this.#paragraphIdsResource.current ?? [];

        if (ids.length === 0) {
            return noop;
        }

        const initialParagraphId = this.#props.initialParagraphId;

        // Kick the originals fetch as soon as paragraph IDs are known.
        // Front-load the prefix containing the restore target so layout
        // can settle before background-filling the rest. Idempotent —
        // tracked by id-list length so a re-fire on the same chapter is
        // a no-op.
        if (this.#originalsKickedFor !== ids.length) {
            this.#originalsKickedFor = ids.length;
            const targetIdx = initialParagraphId != null
                ? ids.indexOf(initialParagraphId)
                : 0;
            const safeTargetIdx = Math.max(targetIdx, 0);
            const headEnd = Math.min(
                safeTargetIdx + RESTORE_PREFIX_BUFFER,
                ids.length,
            );
            this.#store.enqueueOriginals(ids.slice(0, headEnd));
            if (headEnd < ids.length) {
                this.#store.enqueueOriginals(ids.slice(headEnd));
            }
            // Eager translations for an estimated page-worth on each
            // side of the target. Sized from the container's
            // clientHeight rather than a fixed number, so taller
            // viewports get proportionally more. The IPC overlaps the
            // originals fetch instead of trailing it. After the page is
            // visible, #recomputeMountWindow (in tryReveal /
            // #finishRestore) enqueues any further visible paragraphs;
            // the store dedups.
            const pageHeight = this.#props.container?.clientHeight ?? 600;
            const paragraphsPerPage = Math.max(
                5,
                Math.ceil(pageHeight / ESTIMATED_PARAGRAPH_HEIGHT_PX),
            );
            const eagerStart = Math.max(safeTargetIdx - paragraphsPerPage, 0);
            const eagerEnd = Math.min(
                safeTargetIdx + paragraphsPerPage + 1,
                ids.length,
            );
            this.#store.enqueueTranslations(
                ids.slice(eagerStart, eagerEnd),
            );
        }

        if (this.#initialParagraphSyncedFor === initialParagraphId) {
            return noop;
        }

        if (initialParagraphId == null) {
            const firstId = ids[0];
            this.#setVisibleParagraph(firstId, 0);
            this.#initialParagraphSyncedFor = null;
            // Hook is installed synchronously — the effect can re-run
            // (paragraphIds settling, container ref binding) and we
            // can't await anything here without racing the effect's
            // cleanup. The hook itself is idempotent: it short-circuits
            // once the gate has lifted.
            //
            // We don't pre-emptively compute the mount window — wrappers
            // are still empty placeholders, so every paragraph would
            // pass the geometric threshold and the resulting (whole-
            // chapter) translations enqueue would back up on the
            // shared backend book lock and starve the originals fetch.
            // Defer the first measurement until firstId has real text
            // height (i.e. its original has landed in the store).
            const tryReveal = () => {
                if (
                    this.#isInitiallyReady ||
                    this.#initialRevealRaf !== null
                ) {
                    return;
                }
                if (!this.#readyParagraphIds.has(firstId)) return;
                this.#recomputeMountWindow();
                this.#initialRevealRaf = requestAnimationFrame(() => {
                    this.#initialRevealRaf = null;
                    this.#markInitiallyReady();
                });
            };
            this.#noRestoreRevealHook = tryReveal;
            tryReveal();
            return noop;
        }

        if (!this.#props.container) {
            return noop;
        }

        const paragraphIdToScrollTo = initialParagraphId;
        const pageOffsetToRestore = Math.max(0, this.#props.initialPageOffset | 0);
        this.#initialParagraphSyncedFor = paragraphIdToScrollTo;

        // Prime the visible/saved trackers so any onscroll noise leaking
        // past #isRestoring can't overwrite the persisted state with an
        // intermediate position.
        this.#visibleParagraphId = paragraphIdToScrollTo;
        this.#visiblePageOffset = pageOffsetToRestore;
        this.#lastSavedParagraph = paragraphIdToScrollTo;
        this.#lastSavedPageOffset = pageOffsetToRestore;
        this.#isRestoring = true;
        this.#restoreTarget = paragraphIdToScrollTo;
        this.#restorePageOffset = pageOffsetToRestore;

        // Suspend native scroll-snap for the volatile period: as
        // paragraphs land and column count grows, snap targets get
        // re-rendered, and the browser could yank scrollLeft to a
        // stale target between #anchorToParagraph calls. #finishRestore
        // re-enables it once the layout has settled.
        const container = this.#props.container;
        if (container) {
            this.#savedSnapType = container.style.scrollSnapType;
            container.style.scrollSnapType = "none";
        }

        // First anchor — likely off because per-paragraph fetches haven't
        // populated yet. registerParagraphReady and the ResizeObserver
        // re-anchor as data and layout land.
        this.#anchorToParagraph(paragraphIdToScrollTo);

        // Catch-all for layout shifts that aren't tied to a paragraph
        // fetch (late font load, image dimensions, column-flow reflow
        // after a wrapper finishes). Each child wrapper's resize feeds
        // the same coalesced re-anchor as the ready signal, and also
        // refreshes the column-count so the snap targets keep pace.
        if (container && typeof ResizeObserver !== "undefined") {
            const observer = new ResizeObserver(() => {
                this.#scheduleAnchorRaf();
                this.#scheduleColumnCountRecompute();
            });
            for (let i = 0; i < container.children.length; i++) {
                observer.observe(container.children[i] as HTMLElement);
            }
            this.#restoreResizeObserver = observer;
        }

        if (this.#readyThroughRestoreTarget()) {
            // The visible region is already settled (cached or pre-fetched).
            // Still go through the scheduled-anchor path so the deferred
            // final anchor runs.
            this.#scheduleAnchorRaf();
        } else {
            this.#restoreFallbackTimeout = setTimeout(() => {
                this.#restoreFallbackTimeout = null;
                if (this.#restoreTarget != null) {
                    this.#anchorToParagraph(this.#restoreTarget);
                }
                this.#finishRestore();
            }, RESTORE_FALLBACK_MS);
        }

        // No $effect cleanup here. The $effect can re-run with identical
        // deps (Svelte fires it on parent re-renders even when our prop
        // values are unchanged). Aborting the in-flight restore in that
        // window leaves the chapter scrolled to a partial-layout anchor.
        // The actual lifecycle teardown (ChapterView unmount) is handled
        // by vm.dispose() → #finishRestore() in onDestroy. The
        // #initialParagraphSyncedFor guard above already prevents a
        // second restore from being kicked off if the effect re-fires
        // with the same target.
        return noop;
    }

    registerParagraphReady(paragraphId: number): void {
        this.#readyParagraphIds.add(paragraphId);
        // Each paragraph's data lands → its height grows → scrollWidth
        // grows → snap targets need to cover the new columns. Coalesced
        // via rAF so a batch of readies in one frame is a single recompute.
        this.#scheduleColumnCountRecompute();
        if (this.#restoreTarget == null) {
            // No-restore path is waiting on the first paragraph's data
            // to lift the opacity gate.
            this.#noRestoreRevealHook?.();
            return;
        }
        this.#scheduleAnchorRaf();
    }

    #readyThroughRestoreTarget(): boolean {
        const target = this.#restoreTarget;
        if (target == null) return false;
        // Paragraphs after the target sit in columns to the right of the
        // visible page and can't shift it, so we only need everything up
        // through the target to be settled before lifting the gate.
        const ids = this.#paragraphIdsResource.current ?? [];
        for (const id of ids) {
            if (!this.#readyParagraphIds.has(id)) return false;
            if (id === target) return true;
        }
        return false;
    }

    #scheduleAnchorRaf(): void {
        if (this.#restoreTarget == null) return;
        // Coalesce — many ready events and ResizeObserver fires can
        // arrive in the same frame; one rAF handles them all.
        if (this.#anchorRaf !== null) {
            return;
        }
        this.#anchorRaf = requestAnimationFrame(() => {
            this.#anchorRaf = null;
            if (this.#restoreTarget == null) return;
            this.#anchorToParagraph(this.#restoreTarget);
            if (this.#readyThroughRestoreTarget()) {
                // Defer one more rAF: a paragraph's onReady fires from a
                // Svelte $effect, which runs before the browser's next
                // layout phase reflows the column flow. One extra frame
                // ensures the final anchor reads the fully-settled rect.
                const target = this.#restoreTarget;
                this.#anchorRaf = requestAnimationFrame(() => {
                    this.#anchorRaf = null;
                    if (this.#restoreTarget == null) return;
                    this.#anchorToParagraph(target);
                    this.#finishRestore();
                });
            }
        });
    }

    #anchorToParagraph(id: number): void {
        const container = this.#props.container;
        const target = this.#findParagraphWrapper(id);
        if (!container || !target) {
            return;
        }
        const containerRect = container.getBoundingClientRect();
        const targetRect = target.getBoundingClientRect();
        // Left-align the wrapper's pageOffset-th column with the
        // viewport. For multi-column wrappers, pageOffset picks which
        // column of the wrapper is shown. The computed scrollLeft is
        // exactly column-aligned, and handleScrollEnd is gated by
        // #isRestoring, so nothing perturbs the position.
        const desiredScrollLeft =
            container.scrollLeft +
            (targetRect.left - containerRect.left) +
            this.#restorePageOffset * containerRect.width;
        container.scrollTo({ left: desiredScrollLeft, behavior: "auto" });
    }

    #finishRestore(): void {
        if (this.#anchorRaf !== null) {
            cancelAnimationFrame(this.#anchorRaf);
            this.#anchorRaf = null;
        }
        if (this.#restoreFallbackTimeout !== null) {
            clearTimeout(this.#restoreFallbackTimeout);
            this.#restoreFallbackTimeout = null;
        }
        if (this.#restoreResizeObserver !== null) {
            this.#restoreResizeObserver.disconnect();
            this.#restoreResizeObserver = null;
        }
        const wasRestoring = this.#restoreTarget != null;
        this.#restoreTarget = null;
        this.#isRestoring = false;

        // Re-enable scroll-snap. Our anchor already left scrollLeft at
        // a column boundary that matches a snap target, so the browser
        // does not need to move anything; subsequent user scrolls will
        // pick up native snap behavior naturally.
        const container = this.#props.container;
        if (container && this.#savedSnapType !== null) {
            container.style.scrollSnapType = this.#savedSnapType;
            this.#savedSnapType = null;
        }

        if (wasRestoring) {
            this.#recomputeMountWindow();
            this.#recomputeColumnCount();
        }

        // Snap is re-engaged and the final anchor has landed: it's safe
        // to lift the opacity gate. Reveal-path call sites are idempotent
        // via #markInitiallyReady's early-return.
        this.#markInitiallyReady();
    }

    #markInitiallyReady(): void {
        if (this.#isInitiallyReady) return;
        this.#isInitiallyReady = true;
        if (this.#initialRevealFallbackTimeout !== null) {
            clearTimeout(this.#initialRevealFallbackTimeout);
            this.#initialRevealFallbackTimeout = null;
        }
        if (this.#initialRevealRaf !== null) {
            cancelAnimationFrame(this.#initialRevealRaf);
            this.#initialRevealRaf = null;
        }
        this.#noRestoreRevealHook = null;
    }

    dispose(): void {
        if (this.#scrollRaf !== null) {
            cancelAnimationFrame(this.#scrollRaf);
            this.#scrollRaf = null;
        }
        if (this.#columnCountRaf !== null) {
            cancelAnimationFrame(this.#columnCountRaf);
            this.#columnCountRaf = null;
        }
        if (this.#initialRevealFallbackTimeout !== null) {
            clearTimeout(this.#initialRevealFallbackTimeout);
            this.#initialRevealFallbackTimeout = null;
        }
        if (this.#initialRevealRaf !== null) {
            cancelAnimationFrame(this.#initialRevealRaf);
            this.#initialRevealRaf = null;
        }
        this.#noRestoreRevealHook = null;
        this.#finishRestore();
        if (this.#saveTimeout) {
            clearTimeout(this.#saveTimeout);
            this.#saveTimeout = null;
        }
        if (this.#resizeTimeout) {
            clearTimeout(this.#resizeTimeout);
            this.#resizeTimeout = null;
        }
        if (
            this.#visibleParagraphId != null &&
            (this.#lastSavedParagraph !== this.#visibleParagraphId ||
                this.#lastSavedPageOffset !== this.#visiblePageOffset)
        ) {
            this.#props.onPositionChange?.(
                this.#visibleParagraphId,
                this.#visiblePageOffset,
            );
        }
    }

    #updateVisibleParagraph(): void {
        const next = this.#findVisibleParagraph();
        if (next != null) {
            this.#setVisibleParagraph(next.id, next.pageOffset);
        }
        this.#recomputeMountWindow();
    }

    #recomputeMountWindow(): void {
        const container = this.#props.container;
        if (!container) {
            return;
        }
        const containerRect = container.getBoundingClientRect();
        const pageWidth = containerRect.width;
        if (pageWidth <= 0) {
            return;
        }
        const children = container.children;
        if (children.length === 0) {
            if (this.#mountedParagraphIds.size !== 0) {
                this.#mountedParagraphIds = new Set();
            }
            return;
        }

        // One pass: read all geometry, locate the visible paragraph index.
        // We use getBoundingClientRect rather than offsetLeft because in a CSS
        // multi-column flow offsetLeft is unreliable across engines, while
        // bounding rect reflects the actual visual layout.
        const scrollLeft = container.scrollLeft;
        const wrappers: Array<{ id: number; center: number }> = [];
        let visibleIdx = -1;
        for (let i = 0; i < children.length; i++) {
            const child = children[i] as HTMLElement;
            const idAttr = child.dataset["paragraphId"];
            if (idAttr == null) {
                continue;
            }
            const id = parseInt(idAttr, 10);
            if (Number.isNaN(id)) {
                continue;
            }
            const rect = child.getBoundingClientRect();
            // Position in the container's content coordinate system
            // (independent of current scroll position).
            const center =
                rect.left - containerRect.left + scrollLeft + rect.width / 2;
            wrappers.push({ id, center });
            if (id === this.#visibleParagraphId) {
                visibleIdx = wrappers.length - 1;
            }
        }
        if (wrappers.length === 0) {
            return;
        }
        if (visibleIdx < 0) {
            visibleIdx = 0;
        }
        const visibleCenter = wrappers[visibleIdx].center;

        const next = new Set<number>();
        for (let i = 0; i < wrappers.length; i++) {
            const { id, center } = wrappers[i];
            const siblingDist = Math.abs(i - visibleIdx);
            if (siblingDist <= SIBLING_RADIUS) {
                next.add(id);
                continue;
            }
            const geomDist = Math.abs(center - visibleCenter) / pageWidth;
            const wasMounted = this.#mountedParagraphIds.has(id);
            let mount: boolean;
            if (geomDist <= GEOM_MOUNT_THRESHOLD) {
                mount = true;
            } else if (geomDist > GEOM_UNMOUNT_THRESHOLD) {
                mount = false;
            } else {
                mount = wasMounted; // hysteresis band
            }
            if (mount) {
                next.add(id);
            }
        }

        if (!setsEqual(next, this.#mountedParagraphIds)) {
            this.#mountedParagraphIds = next;
            // The mount window is the sole driver of translation
            // fetches. Translations for paragraphs leaving the window
            // stay cached; new entrants are enqueued (dedup'd against
            // cached + already-enqueued ids by the store).
            this.#store.enqueueTranslations([...next]);
        }
    }

    #setVisibleParagraph(paragraphId: number, pageOffset: number): void {
        if (
            this.#visibleParagraphId === paragraphId &&
            this.#visiblePageOffset === pageOffset
        ) {
            return;
        }
        this.#visibleParagraphId = paragraphId;
        this.#visiblePageOffset = pageOffset;
        this.#scheduleSave(paragraphId, pageOffset);
    }

    #scheduleSave(paragraphId: number, pageOffset: number): void {
        if (this.#saveTimeout) {
            clearTimeout(this.#saveTimeout);
        }

        this.#saveTimeout = setTimeout(() => {
            if (
                this.#lastSavedParagraph === paragraphId &&
                this.#lastSavedPageOffset === pageOffset
            ) {
                return;
            }
            this.#lastSavedParagraph = paragraphId;
            this.#lastSavedPageOffset = pageOffset;
            this.#props.onPositionChange?.(paragraphId, pageOffset);
        }, 400);
    }

    #findVisibleParagraph(): { id: number; pageOffset: number } | null {
        const container = this.#props.container;
        if (!container) {
            return null;
        }
        const containerRect = container.getBoundingClientRect();
        // Hit-test the top-left of the visible column. On touch where
        // break-inside is auto, the same wrapper can span multiple
        // columns; pageOffset records which of those columns the user
        // is on so restore can land on the same one.
        const x = containerRect.left + 16;
        const y = containerRect.top + 16;
        const hit = document.elementFromPoint(x, y) as HTMLElement | null;
        const wrapper = hit?.closest<HTMLElement>(".paragraph-wrapper") ?? null;
        const idAttr = wrapper?.dataset["paragraphId"];
        if (!wrapper || !idAttr) {
            return null;
        }
        const id = parseInt(idAttr, 10);
        if (Number.isNaN(id)) {
            return null;
        }
        const wrapperRect = wrapper.getBoundingClientRect();
        const columnWidth = containerRect.width;
        const pageOffset = columnWidth > 0
            ? Math.max(
                  0,
                  Math.round(
                      (containerRect.left - wrapperRect.left) / columnWidth,
                  ),
              )
            : 0;
        return { id, pageOffset };
    }

    #findParagraphWrapper(paragraphId: number): HTMLElement | null {
        const container = this.#props.container;
        if (!container) {
            return null;
        }
        const targetId = String(paragraphId);
        const children = container.children;
        for (let i = 0; i < children.length; i++) {
            const child = children[i] as HTMLElement;
            if (child.dataset["paragraphId"] === targetId) {
                return child;
            }
        }
        return null;
    }

    #scrollParagraphIntoView(
        paragraphId: number,
        options: ScrollIntoViewOptions = {
            behavior: "auto",
            block: "nearest",
            inline: "center",
        },
    ): boolean {
        const target = this.#findParagraphWrapper(paragraphId);
        if (!target) {
            return false;
        }
        target.scrollIntoView(options);
        return true;
    }
}

function setsEqual(a: Set<number>, b: Set<number>): boolean {
    if (a.size !== b.size) return false;
    for (const v of a) {
        if (!b.has(v)) return false;
    }
    return true;
}

function noop(): void {}
