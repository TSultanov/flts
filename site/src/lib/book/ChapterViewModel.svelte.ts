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
    onPositionChange?: (
        chapterId: number,
        paragraphId: number,
        pageOffset: number,
    ) => void;
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
// Restore normally settles when every paragraph reports loaded; these caps
// only fire if a fetch never resolves, so the user isn't stranded at a
// partial scroll position or a blank (opacity-gated) panel.
const RESTORE_FALLBACK_MS = 3000;
const INITIAL_REVEAL_FALLBACK_MS = 1500;
// Originals fetched past the restore target so snap overshoot still has
// laid-out paragraphs.
const RESTORE_PREFIX_BUFFER = 10;
// Mid-range paragraph height: dialog ~32 px, prose 128-160 px.
const ESTIMATED_PARAGRAPH_HEIGHT_PX = 100;

export class ChapterViewModel {
    #library!: Library;
    #props!: ChapterVMProps;
    // Captured non-reactively: BookView's {#key chapterId} gives this VM
    // exactly one chapter, and the parent's reactive chapterId may already
    // have advanced by the time a position flush fires.
    #chapterId!: number;

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

    // WordSpans render only for ids in this set. Empty means "not yet
    // measured" — render eagerly so initial load never flashes plain text.
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

    // Click-to-reveal words keyed `${paragraphId}:${flatIndex}`; ephemeral —
    // initial per-word visibility comes from card familiarity alone.
    #revealedWordKeys: Set<string> = $state(new Set());

    constructor(library: Library, props: ChapterVMProps) {
        this.#library = library;
        this.#props = props;
        this.#chapterId = props.chapterId;
        this.#store = new ChapterParagraphsStore(props.bookId, library);
        // Lifts the opacity gate if neither reveal path ever fires.
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
        // ceil: a partial column still needs a snap target at its start.
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
        // New Set instance so `$state` notices the change.
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

        // Front-load the prefix through the restore target so layout settles
        // before the rest fills in. Keyed on id-count to stay idempotent.
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
            // A viewport-sized window of translations on each side of the
            // target, so the IPC overlaps the originals fetch rather than
            // trailing it. #recomputeMountWindow enqueues the rest later.
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

        // Echo guard: our own save flows back down through BookView as a
        // changed initialParagraphId and would kick a disruptive re-restore.
        // Skip targets this VM emitted; a genuine late external seed still
        // gets through, as does the fresh-open (null) path.
        if (
            this.#lastSavedParagraph !== null &&
            initialParagraphId === this.#lastSavedParagraph
        ) {
            this.#initialParagraphSyncedFor = initialParagraphId;
            return noop;
        }

        if (initialParagraphId == null) {
            const firstId = ids[0];
            this.#setVisibleParagraph(firstId, 0);
            this.#initialParagraphSyncedFor = null;
            // Measuring before firstId has real text height would pass every
            // paragraph the geometric threshold; the whole-chapter
            // translations enqueue then backs up on the backend book lock
            // and starves the originals fetch. Hook must be installed
            // synchronously (the effect can re-run); it is idempotent.
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

        // Prime the trackers so scroll noise leaking past #isRestoring can't
        // persist an intermediate position.
        this.#visibleParagraphId = paragraphIdToScrollTo;
        this.#visiblePageOffset = pageOffsetToRestore;
        this.#lastSavedParagraph = paragraphIdToScrollTo;
        this.#lastSavedPageOffset = pageOffsetToRestore;
        this.#isRestoring = true;
        this.#restoreTarget = paragraphIdToScrollTo;
        this.#restorePageOffset = pageOffsetToRestore;

        // Suspend scroll-snap while columns are still growing: the browser
        // would yank scrollLeft to a stale snap target between anchors.
        // #finishRestore re-enables it.
        const container = this.#props.container;
        if (container) {
            this.#savedSnapType = container.style.scrollSnapType;
            container.style.scrollSnapType = "none";
        }

        // Approximate — registerParagraphReady and the ResizeObserver
        // re-anchor as data and layout land.
        this.#anchorToParagraph(paragraphIdToScrollTo);

        // Catches layout shifts not tied to a paragraph fetch: late fonts,
        // image dimensions, column reflow.
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
            // Already settled, but go through the scheduled path so the
            // deferred final anchor still runs.
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

        // Deliberately no cleanup: the $effect re-runs on parent re-renders
        // with identical deps, and aborting an in-flight restore there
        // strands the chapter at a partial-layout anchor. Teardown happens
        // via dispose() → #finishRestore() on unmount.
        return noop;
    }

    registerParagraphReady(paragraphId: number): void {
        this.#readyParagraphIds.add(paragraphId);
        // Data landing grows scrollWidth, so snap targets must cover the
        // new columns.
        this.#scheduleColumnCountRecompute();
        if (this.#restoreTarget == null) {
            this.#noRestoreRevealHook?.();
            return;
        }
        this.#scheduleAnchorRaf();
    }

    #readyThroughRestoreTarget(): boolean {
        const target = this.#restoreTarget;
        if (target == null) return false;
        // Paragraphs past the target sit in columns to the right and can't
        // shift the visible page.
        const ids = this.#paragraphIdsResource.current ?? [];
        for (const id of ids) {
            if (!this.#readyParagraphIds.has(id)) return false;
            if (id === target) return true;
        }
        return false;
    }

    #scheduleAnchorRaf(): void {
        if (this.#restoreTarget == null) return;
        // Coalesce a frame's worth of ready/resize events into one anchor.
        if (this.#anchorRaf !== null) {
            return;
        }
        this.#anchorRaf = requestAnimationFrame(() => {
            this.#anchorRaf = null;
            if (this.#restoreTarget == null) return;
            this.#anchorToParagraph(this.#restoreTarget);
            if (this.#readyThroughRestoreTarget()) {
                // onReady fires from a $effect, before the browser reflows
                // the column flow; one extra frame gets a settled rect.
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
        // Left-align the wrapper's pageOffset-th column with the viewport;
        // for a wrapper spanning columns, pageOffset picks which one.
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

        // Safe to re-enable: the final anchor left scrollLeft on a column
        // boundary, so the browser has nothing to move.
        const container = this.#props.container;
        if (container && this.#savedSnapType !== null) {
            container.style.scrollSnapType = this.#savedSnapType;
            this.#savedSnapType = null;
        }

        if (wasRestoring) {
            this.#recomputeMountWindow();
            this.#recomputeColumnCount();
        }

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
        this.#store.dispose();
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
                this.#chapterId,
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

        // getBoundingClientRect, not offsetLeft: offsetLeft is unreliable
        // across engines in a CSS multi-column flow.
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
            // Content coordinates, independent of current scroll.
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
            // Sole driver of translation fetches; the store dedups.
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
            this.#props.onPositionChange?.(
                this.#chapterId,
                paragraphId,
                pageOffset,
            );
        }, 400);
    }

    #findVisibleParagraph(): { id: number; pageOffset: number } | null {
        const container = this.#props.container;
        if (!container) {
            return null;
        }
        const containerRect = container.getBoundingClientRect();
        // Hit-test the visible column's top-left. A wrapper can span several
        // columns, so pageOffset records which one, for restore.
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
