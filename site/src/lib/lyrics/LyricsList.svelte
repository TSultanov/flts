<script lang="ts">
    import LyricsLine from './LyricsLine.svelte';
    import type {
        Lyrics,
        LyricsTranslation,
        NowPlaying,
    } from './types';

    type Props = {
        lyrics: Lyrics | null;
        translation: LyricsTranslation | null;
        nowPlaying: NowPlaying | null;
        livePositionMs: number;
    };

    let { lyrics, translation, nowPlaying, livePositionMs }: Props = $props();

    const activeIndex = $derived.by(() => {
        if (!lyrics || !lyrics.synced) return -1;
        if (
            !nowPlaying ||
            nowPlaying.state === 'notrunning' ||
            nowPlaying.state === 'stopped'
        ) {
            return -1;
        }
        // Lines are sorted by time_ms ascending; find the last entry with time_ms <= pos.
        const pos = livePositionMs;
        const lines = lyrics.lines;
        let lo = 0;
        let hi = lines.length;
        let candidate = -1;
        let hasNullTimes = false;
        while (lo < hi) {
            const mid = (lo + hi) >> 1;
            const t = lines[mid].time_ms;
            if (t === null) {
                hasNullTimes = true;
                break;
            }
            if (t <= pos) {
                candidate = mid;
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        if (hasNullTimes) {
            candidate = -1;
            for (let i = 0; i < lines.length; i++) {
                const t = lines[i].time_ms;
                if (t === null) continue;
                if (t <= pos) candidate = i;
                else break;
            }
        }
        return candidate;
    });

    let listEl: HTMLDivElement | undefined = $state();
    let lineEls: (HTMLElement | null)[] = $state([]);

    $effect(() => {
        const i = activeIndex;
        const el = lineEls[i];
        if (!el || !listEl) return;
        const containerRect = listEl.getBoundingClientRect();
        const elRect = el.getBoundingClientRect();
        // Center the line if it's outside the middle 50% of the container.
        const topZone = containerRect.top + containerRect.height * 0.25;
        const bottomZone = containerRect.top + containerRect.height * 0.75;
        if (elRect.top < topZone || elRect.bottom > bottomZone) {
            el.scrollIntoView({ behavior: 'smooth', block: 'center' });
        }
    });
</script>

<div class="list" bind:this={listEl}>
    {#if !lyrics}
        <div class="empty">No lyrics loaded.</div>
    {:else if lyrics.lines.length === 0}
        <div class="empty">Lyrics are empty.</div>
    {:else}
        {#each lyrics.lines as line, i}
            <div bind:this={lineEls[i]}>
                <LyricsLine
                    {line}
                    translation={translation?.lines[i]}
                    active={i === activeIndex}
                />
            </div>
        {/each}
    {/if}
</div>

<style>
    .list {
        height: 100%;
        overflow-y: auto;
        padding: 12px 0 40vh 0; /* trailing space so last line can center */
    }
    .empty {
        padding: 30px;
        text-align: center;
        font-style: italic;
        opacity: 0.6;
    }
</style>
