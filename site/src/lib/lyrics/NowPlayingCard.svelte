<script lang="ts">
    import type { NowPlaying } from './types';
    import type { TrackMeta } from '../spotify/queueStore';

    type Props = {
        nowPlaying: NowPlaying | null;
        livePositionMs: number;
        nextTrack?: TrackMeta | null;
    };

    let { nowPlaying, livePositionMs, nextTrack = null }: Props = $props();

    function fmtMs(ms?: number): string {
        if (ms === undefined || ms === null) return '–:––';
        const total = Math.max(0, Math.floor(ms / 1000));
        const m = Math.floor(total / 60);
        const s = total % 60;
        return `${m}:${s.toString().padStart(2, '0')}`;
    }
</script>

<div class="card">
    {#if !nowPlaying || nowPlaying.state === 'notrunning'}
        <div class="status">Spotify is not running</div>
    {:else if nowPlaying.state === 'stopped'}
        <div class="status">Spotify is stopped</div>
    {:else}
        <div class="track">
            <div class="title">{nowPlaying.name ?? ''}</div>
            <div class="meta">
                <span class="artist">{nowPlaying.artist ?? ''}</span>
                {#if nowPlaying.album}
                    <span class="dot">·</span>
                    <span class="album">{nowPlaying.album}</span>
                {/if}
            </div>
            <div class="state">
                <span class="state-indicator">
                    {nowPlaying.state === 'playing' ? '▶' : '⏸'}
                </span>
                <span class="time">
                    {fmtMs(livePositionMs)} / {fmtMs(nowPlaying.durationMs)}
                </span>
            </div>
        </div>
        {#if nextTrack}
            <div class="next-track">
                <div class="next-label">Up next</div>
                <div class="next-title">{nextTrack.name}</div>
                <div class="next-artist">{nextTrack.artist}</div>
            </div>
        {/if}
    {/if}
</div>

<style>
    .card {
        padding: 14px 18px;
        background-color: var(--background-color);
        color: var(--text-inverted);
        border-bottom: 1px solid var(--background-color);
        display: grid;
        grid-template-columns: 1fr auto;
        align-items: center;
        column-gap: 24px;
    }
    .status {
        font-style: italic;
        opacity: 0.85;
        grid-column: 1 / -1;
    }
    .track {
        grid-column: 1 / 2;
        min-width: 0;
    }
    .title {
        font-size: 1.2em;
        font-weight: 600;
    }
    .meta {
        margin-top: 2px;
        font-size: 0.95em;
        opacity: 0.85;
    }
    .dot {
        margin: 0 6px;
        opacity: 0.5;
    }
    .state {
        margin-top: 6px;
        font-size: 0.85em;
        opacity: 0.7;
        display: flex;
        gap: 10px;
        align-items: center;
    }
    .state-indicator {
        font-size: 1em;
    }
    .next-track {
        grid-column: 2 / 3;
        text-align: right;
        opacity: 0.65;
        font-size: 0.85em;
        max-width: 240px;
        min-width: 0;
    }
    .next-label {
        text-transform: uppercase;
        letter-spacing: 0.05em;
        font-size: 0.75em;
        opacity: 0.7;
        margin-bottom: 2px;
    }
    .next-title {
        font-weight: 500;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    .next-artist {
        opacity: 0.75;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
</style>
