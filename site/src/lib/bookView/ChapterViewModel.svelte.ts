import { tick } from "svelte";
import type { Library } from "../data/library";
import type { UUID } from "../data/v2/db";
import type { WordSelection } from "./ParagraphViewModel.svelte";

export type ChapterVMProps = {
    bookId: UUID;
    chapterId: number;
    initialParagraphId: number | null;
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
    #saveTimeout: ReturnType<typeof setTimeout> | null = null;
    #lastSavedParagraph: number | null = null;
    #isResizing = false;
    #resizeTimeout: ReturnType<typeof setTimeout> | null = null;
    #scrollRaf: number | null = null;
    #initialParagraphSyncedFor: number | null | undefined = undefined;
    #isRestoring = false;
    #readyParagraphIds = new Set<number>();
    #restoreTarget: number | null = null;
    #restoreFallbackTimeout: ReturnType<typeof setTimeout> | null = null;
    #anchorRaf: number | null = null;

    constructor(library: Library, props: ChapterVMProps) {
        this.#library = library;
        this.#props = props;
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
            this.#setVisibleParagraph(ids[0]);
            this.#initialParagraphSyncedFor = null;
            const controller = new AbortController();
            void (async () => {
                await tick();
                if (!controller.signal.aborted) {
                    this.#recomputeMountWindow();
                }
            })();
            return () => controller.abort();
        }

        if (!this.#props.container) {
            return noop;
        }

        const paragraphIdToScrollTo = initialParagraphId;
        this.#initialParagraphSyncedFor = paragraphIdToScrollTo;
        const controller = new AbortController();

        // Prime the visible/saved trackers so any onscroll noise leaking
        // past #isRestoring can't overwrite the persisted state with an
        // intermediate position.
        this.#visibleParagraphId = paragraphIdToScrollTo;
        this.#lastSavedParagraph = paragraphIdToScrollTo;
        this.#isRestoring = true;
        this.#restoreTarget = paragraphIdToScrollTo;

        // First anchor — likely off because per-paragraph fetches haven't
        // populated yet. registerParagraphReady re-anchors as data lands.
        this.#anchorToParagraph(paragraphIdToScrollTo);

        if (this.#readyParagraphIds.size >= ids.length) {
            // Everything was already ready (cached). Finalize on next frame
            // so DOM has flushed any pending heights.
            this.#anchorRaf = requestAnimationFrame(() => {
                this.#anchorRaf = null;
                this.#anchorToParagraph(paragraphIdToScrollTo);
                this.#finishRestore();
            });
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
            return;
        }

        // Coalesce — many paragraphs can flip ready in the same frame.
        if (this.#anchorRaf !== null) {
            return;
        }
        this.#anchorRaf = requestAnimationFrame(() => {
            this.#anchorRaf = null;
            if (this.#restoreTarget == null) return;
            this.#anchorToParagraph(this.#restoreTarget);
            const total = this.#paragraphIdsResource.current?.length ?? 0;
            if (total > 0 && this.#readyParagraphIds.size >= total) {
                this.#finishRestore();
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
        const desiredScrollLeft =
            container.scrollLeft +
            (targetRect.left - containerRect.left) +
            (targetRect.width - containerRect.width) / 2;
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
        const wasRestoring = this.#restoreTarget != null;
        this.#restoreTarget = null;
        this.#isRestoring = false;
        if (wasRestoring) {
            this.#recomputeMountWindow();
        }
    }

    dispose(): void {
        if (this.#scrollRaf !== null) {
            cancelAnimationFrame(this.#scrollRaf);
            this.#scrollRaf = null;
        }
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
            this.#lastSavedParagraph !== this.#visibleParagraphId
        ) {
            this.#library
                .saveBookReadingState(
                    this.#props.bookId,
                    this.#props.chapterId,
                    this.#visibleParagraphId,
                )
                .catch((err) =>
                    console.error("Failed to save reading state", err),
                );
        }
    }

    #updateVisibleParagraph(): void {
        const nextParagraph = this.#findVisibleParagraph();
        if (nextParagraph != null) {
            this.#setVisibleParagraph(nextParagraph);
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

    #setVisibleParagraph(paragraphId: number): void {
        if (this.#visibleParagraphId === paragraphId) {
            return;
        }
        this.#visibleParagraphId = paragraphId;
        this.#scheduleSave(paragraphId);
    }

    #scheduleSave(paragraphId: number): void {
        if (this.#saveTimeout) {
            clearTimeout(this.#saveTimeout);
        }

        this.#saveTimeout = setTimeout(() => {
            if (this.#lastSavedParagraph === paragraphId) {
                return;
            }
            this.#lastSavedParagraph = paragraphId;
            this.#library
                .saveBookReadingState(
                    this.#props.bookId,
                    this.#props.chapterId,
                    paragraphId,
                )
                .catch((err) =>
                    console.error("Failed to save reading state", err),
                );
        }, 400);
    }

    #findVisibleParagraph(): number | null {
        const container = this.#props.container;
        if (!container) {
            return null;
        }
        const containerRect = container.getBoundingClientRect();
        // Hit-test the top-left of the visible column — wrappers have
        // break-inside: avoid, so this lands on the topmost paragraph in
        // the current page and is what gets persisted as the reader's
        // position.
        const x = containerRect.left + 16;
        const y = containerRect.top + 16;
        const hit = document.elementFromPoint(x, y) as HTMLElement | null;
        const wrapper = hit?.closest<HTMLElement>(".paragraph-wrapper") ?? null;
        const idAttr = wrapper?.dataset["paragraphId"];
        if (!idAttr) {
            return null;
        }
        const id = parseInt(idAttr, 10);
        return Number.isNaN(id) ? null : id;
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
