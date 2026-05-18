<script lang="ts">
    import { getContext, onDestroy, onMount, tick } from "svelte";
    import type { UUID } from "../data/v2/db";
    import ParagraphView from "./ParagraphView.svelte";
    import type { Library } from "../data/library";
    import type { WordSelection } from "./ParagraphViewModel.svelte";

    let {
        selection = $bindable(),
        bookId,
        chapterId,
        initialParagraphId = null,
    }: {
        selection: WordSelection | null;
        bookId: UUID;
        chapterId: number;
        initialParagraphId?: number | null;
    } = $props();

    const library: Library = getContext("library");
    const paragraphIds = $derived(
        library.getBookChapterParagraphIds(bookId, chapterId),
    );

    function handleWordClick(info: {
        paragraphId: number;
        sentence: number;
        word: number;
        flatIndex: number;
    }) {
        selection = {
            paragraphId: info.paragraphId,
            sentence: info.sentence,
            word: info.word,
        };
        library
            .markWordVisible(bookId, info.paragraphId, info.flatIndex)
            .catch((err) =>
                console.error("Failed to mark word visible", err),
            );
    }

    function handleBackgroundClick(e: MouseEvent) {
        const target = e.target instanceof Element ? e.target : null;
        if (target?.closest(".word-span")) return;
        selection = null;
    }

    let sectionContentWidth = $state(200);
    let paragraphsContainer: HTMLDivElement | null = null;
    let visibleParagraphId: number | null = null;
    let saveTimeout: ReturnType<typeof setTimeout> | null = null;
    let lastSavedParagraph: number | null = null;
    let isResizing = false;
    let resizeTimeout: ReturnType<typeof setTimeout> | null = null;
    let scrollRaf: number | null = null;
    let initialParagraphSyncedFor: number | null | undefined = undefined;

    // Per-paragraph mount gate. WordSpans only render for ids in this set.
    // An empty set means "not yet computed" — paragraphs render eagerly
    // until the first window measurement lands so we never flash plain text
    // on initial load. Once populated it is authoritative.
    let mountedParagraphIds: Set<number> = $state(new Set());
    const SIBLING_RADIUS = 2;
    const GEOM_MOUNT_THRESHOLD = 2.0;
    const GEOM_UNMOUNT_THRESHOLD = 2.5;

    function isMounted(paragraphId: number): boolean {
        return mountedParagraphIds.size === 0
            || mountedParagraphIds.has(paragraphId);
    }

    function handleScroll() {
        if (isResizing) {
            return;
        }
        if (scrollRaf !== null) {
            return;
        }
        scrollRaf = requestAnimationFrame(() => {
            scrollRaf = null;
            updateVisibleParagraph();
        });
    }

    function scheduleSave(paragraphId: number) {
        if (saveTimeout) {
            clearTimeout(saveTimeout);
        }

        saveTimeout = setTimeout(() => {
            if (lastSavedParagraph === paragraphId) {
                return;
            }
            lastSavedParagraph = paragraphId;
            library
                .saveBookReadingState(bookId, chapterId, paragraphId)
                .catch((err) =>
                    console.error("Failed to save reading state", err),
                );
        }, 400);
    }

    function updateVisibleParagraph() {
        const nextParagraph = findVisibleParagraph();
        if (nextParagraph != null) {
            setVisibleParagraph(nextParagraph);
        }
        recomputeMountWindow();
    }

    function recomputeMountWindow() {
        if (!paragraphsContainer) {
            return;
        }
        const containerRect = paragraphsContainer.getBoundingClientRect();
        const pageWidth = containerRect.width;
        if (pageWidth <= 0) {
            return;
        }
        const children = paragraphsContainer.children;
        if (children.length === 0) {
            if (mountedParagraphIds.size !== 0) {
                mountedParagraphIds = new Set();
            }
            return;
        }

        // One pass: read all geometry, locate the visible paragraph index.
        // We use getBoundingClientRect rather than offsetLeft because in a CSS
        // multi-column flow offsetLeft is unreliable across engines, while
        // bounding rect reflects the actual visual layout.
        const scrollLeft = paragraphsContainer.scrollLeft;
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
            if (id === visibleParagraphId) {
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
            const wasMounted = mountedParagraphIds.has(id);
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

        if (!setsEqual(next, mountedParagraphIds)) {
            mountedParagraphIds = next;
        }
    }

    function setsEqual(a: Set<number>, b: Set<number>): boolean {
        if (a.size !== b.size) return false;
        for (const v of a) {
            if (!b.has(v)) return false;
        }
        return true;
    }

    function findVisibleParagraph(): number | null {
        if (!paragraphsContainer) {
            return null;
        }
        const containerRect = paragraphsContainer.getBoundingClientRect();
        const x = containerRect.left + 16;
        const y = containerRect.top + containerRect.height / 2;
        const hit = document.elementFromPoint(x, y) as HTMLElement | null;
        const wrapper = hit?.closest<HTMLElement>(".paragraph-wrapper") ?? null;
        const idAttr = wrapper?.dataset["paragraphId"];
        if (!idAttr) {
            return null;
        }
        const id = parseInt(idAttr, 10);
        return Number.isNaN(id) ? null : id;
    }

    function findParagraphWrapper(paragraphId: number): HTMLElement | null {
        if (!paragraphsContainer) {
            return null;
        }
        const targetId = String(paragraphId);
        const children = paragraphsContainer.children;
        for (let i = 0; i < children.length; i++) {
            const child = children[i] as HTMLElement;
            if (child.dataset["paragraphId"] === targetId) {
                return child;
            }
        }
        return null;
    }

    function setVisibleParagraph(paragraphId: number) {
        if (visibleParagraphId === paragraphId) {
            return;
        }
        visibleParagraphId = paragraphId;
        scheduleSave(paragraphId);
    }

    function scrollParagraphIntoView(
        paragraphId: number,
        options: ScrollIntoViewOptions = {
            behavior: "auto",
            block: "nearest",
            inline: "center",
        },
    ): boolean {
        const target = findParagraphWrapper(paragraphId);
        if (!target) {
            return false;
        }
        target.scrollIntoView(options);
        return true;
    }

    $effect(() => {
        const ids = paragraphIds.current ?? [];

        if (ids.length === 0) {
            return;
        }

        if (initialParagraphSyncedFor === initialParagraphId) {
            return;
        }

        if (initialParagraphId == null) {
            setVisibleParagraph(ids[0]);
            initialParagraphSyncedFor = null;
            const controller = new AbortController();
            void (async () => {
                await tick();
                if (!controller.signal.aborted) {
                    recomputeMountWindow();
                }
            })();
            return () => controller.abort();
        }

        if (!paragraphsContainer) {
            return;
        }

        const paragraphIdToScrollTo = initialParagraphId;
        initialParagraphSyncedFor = paragraphIdToScrollTo;
        const controller = new AbortController();

        void (async () => {
            let scrolled = scrollParagraphIntoView(paragraphIdToScrollTo);
            if (!scrolled) {
                await tick();
                if (controller.signal.aborted) {
                    return;
                }
                scrolled = scrollParagraphIntoView(paragraphIdToScrollTo);
            }

            if (controller.signal.aborted) {
                return;
            }

            if (scrolled) {
                setVisibleParagraph(paragraphIdToScrollTo);
            } else if (ids.length > 0) {
                setVisibleParagraph(ids[0]);
            }
            await tick();
            if (!controller.signal.aborted) {
                recomputeMountWindow();
            }
        })();

        return () => controller.abort();
    });

    onMount(() => {
        const listener = () => {
            isResizing = true;
            if (resizeTimeout) {
                clearTimeout(resizeTimeout);
            }

            if (visibleParagraphId != null && paragraphsContainer) {
                scrollParagraphIntoView(visibleParagraphId, {
                    behavior: "auto",
                    block: "center",
                    inline: "center",
                });
            }

            resizeTimeout = setTimeout(() => {
                isResizing = false;
                recomputeMountWindow();
            }, 200);
        };
        window.addEventListener("resize", listener);
        return () => {
            window.removeEventListener("resize", listener);
            if (resizeTimeout) {
                clearTimeout(resizeTimeout);
            }
        };
    });

    onDestroy(() => {
        if (scrollRaf !== null) {
            cancelAnimationFrame(scrollRaf);
            scrollRaf = null;
        }
        if (saveTimeout) {
            clearTimeout(saveTimeout);
        }
        if (
            visibleParagraphId != null &&
            lastSavedParagraph !== visibleParagraphId
        ) {
            library
                .saveBookReadingState(bookId, chapterId, visibleParagraphId)
                .catch((err) =>
                    console.error("Failed to save reading state", err),
                );
        }
    });
</script>

<div class="chapter-container">
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <section class="chapter" onclick={handleBackgroundClick}>
        <div
            class="paragraphs-container"
            style="column-width: {sectionContentWidth}px"
            bind:clientHeight={sectionContentWidth}
            bind:this={paragraphsContainer}
            onscroll={handleScroll}
        >
            {#each paragraphIds.current ?? [] as paragraphId (paragraphId)}
                <ParagraphView
                    {bookId}
                    {paragraphId}
                    {selection}
                    mounted={isMounted(paragraphId)}
                    onWordClick={handleWordClick}
                />
            {/each}
        </div>
    </section>
</div>

<style>
    .chapter-container {
        background-color: var(--hover-color);
        padding: 10px 25px;
        justify-content: center;
        height: 100%;
        overflow: hidden;
    }

    .chapter {
        padding: 1cm 0;
        max-width: 800px;
        margin: 0 auto;
        border: 1px solid var(--background-color);
        background-color: white;
        box-shadow: 2px 2px var(--background-color);
        text-align: justify;
        line-height: 2;
        height: 100%;
    }

    .paragraphs-container {
        width: 100%;
        height: 100%;
        overflow-x: auto;
        scroll-snap-type: x mandatory;
        column-gap: 0;
        column-fill: auto;
        -webkit-column-fill: auto;
    }

    :global(.paragraphs-container > *) {
        scroll-snap-align: center;
        scroll-snap-stop: always;
    }
</style>
