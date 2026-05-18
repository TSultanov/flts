import { tick } from "svelte";
import type { Library } from "../data/library";
import type { UUID } from "../data/v2/db";
import type { WordSelection } from "./ParagraphViewModel.svelte";

export type ChapterVMProps = {
    bookId: UUID;
    chapterId: number;
    initialParagraphId: number | null;
    initialPageOffset: number;
    container: HTMLDivElement | null;
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

export class ChapterViewModel {
    #library!: Library;
    #props!: ChapterVMProps;

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
    #isInitiallyReady = $state(false);
    #initialRevealFallbackTimeout: ReturnType<typeof setTimeout> | null = null;
    #initialRevealRaf: number | null = null;
    #noRestoreRevealHook: (() => void) | null = null;

    constructor(library: Library, props: ChapterVMProps) {
        this.#library = library;
        this.#props = props;
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
        });
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
        }, 200);
    }

    handleWordClick(info: WordClickInfo): WordSelection {
        this.#library
            .markWordVisible(this.#props.bookId, info.paragraphId, info.flatIndex)
            .catch((err) =>
                console.error("Failed to mark word visible", err),
            );
        return {
            paragraphId: info.paragraphId,
            sentence: info.sentence,
            word: info.word,
        };
    }

    startInitialSync(): () => void {
        const ids = this.#paragraphIdsResource.current ?? [];

        if (ids.length === 0) {
            return noop;
        }

        const initialParagraphId = this.#props.initialParagraphId;

        if (this.#initialParagraphSyncedFor === initialParagraphId) {
            return noop;
        }

        if (initialParagraphId == null) {
            const firstId = ids[0];
            this.#setVisibleParagraph(firstId, 0);
            this.#initialParagraphSyncedFor = null;
            const controller = new AbortController();
            void (async () => {
                await tick();
                if (controller.signal.aborted) return;
                this.#recomputeMountWindow();
                // Lift the opacity gate once the first paragraph's data
                // is ready, plus one rAF so paint catches up before we
                // reveal. The hook is re-checked on each
                // registerParagraphReady call until it succeeds.
                const tryReveal = () => {
                    if (
                        controller.signal.aborted ||
                        this.#isInitiallyReady ||
                        this.#initialRevealRaf !== null
                    ) {
                        return;
                    }
                    if (!this.#readyParagraphIds.has(firstId)) return;
                    this.#initialRevealRaf = requestAnimationFrame(() => {
                        this.#initialRevealRaf = null;
                        if (controller.signal.aborted) return;
                        this.#markInitiallyReady();
                    });
                };
                this.#noRestoreRevealHook = tryReveal;
                tryReveal();
            })();
            return () => controller.abort();
        }

        if (!this.#props.container) {
            return noop;
        }

        const paragraphIdToScrollTo = initialParagraphId;
        const pageOffsetToRestore = Math.max(0, this.#props.initialPageOffset | 0);
        this.#initialParagraphSyncedFor = paragraphIdToScrollTo;
        const controller = new AbortController();

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

        // Suspend scroll-snap for the volatile period: snap-corrected
        // scrollTo can land on the wrong column when the layout is mid-
        // flight. #finishRestore re-enables it after the final anchor.
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
        // the same coalesced re-anchor as the ready signal.
        if (container && typeof ResizeObserver !== "undefined") {
            const observer = new ResizeObserver(() => this.#scheduleAnchorRaf());
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

        controller.signal.addEventListener("abort", () => this.#finishRestore());

        return () => controller.abort();
    }

    registerParagraphReady(paragraphId: number): void {
        this.#readyParagraphIds.add(paragraphId);
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
        // viewport. For single-column wrappers (the desktop case) this
        // equals "center the wrapper". For multi-column wrappers (touch
        // / break-inside: auto), pageOffset picks which of the spans is
        // shown. Snap points are at column starts, so the result snaps
        // cleanly when scroll-snap is re-enabled.
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

        // Re-enable snap without forcing a correction at the current
        // scrollLeft. A multi-column wrapper has only one snap point
        // per wrapper, so a forced re-snap can yank us off the saved
        // pageOffset. The next user scroll will pick up snap naturally.
        const container = this.#props.container;
        if (container && this.#savedSnapType !== null) {
            container.style.scrollSnapType = this.#savedSnapType;
            this.#savedSnapType = null;
        }

        if (wasRestoring) {
            this.#recomputeMountWindow();
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
            this.#library
                .saveBookReadingState(
                    this.#props.bookId,
                    this.#props.chapterId,
                    this.#visibleParagraphId,
                    this.#visiblePageOffset,
                )
                .catch((err) =>
                    console.error("Failed to save reading state", err),
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
            this.#library
                .saveBookReadingState(
                    this.#props.bookId,
                    this.#props.chapterId,
                    paragraphId,
                    pageOffset,
                )
                .catch((err) =>
                    console.error("Failed to save reading state", err),
                );
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
