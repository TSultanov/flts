<script lang="ts">
    import type { LyricsLine, LyricsLineTranslation } from './types';

    type Props = {
        line: LyricsLine;
        translation?: LyricsLineTranslation;
        active: boolean;
    };

    let { line, translation, active }: Props = $props();
</script>

<div class="lyric-line" class:active class:empty={line.text.length === 0}>
    <div class="original">
        {#if line.text.length === 0}
            <span class="stanza-break">¶</span>
        {:else}
            {line.text}
        {/if}
    </div>
    {#if translation && translation.translation.length > 0}
        <div class="translation">{translation.translation}</div>
    {/if}
    {#if translation && translation.glosses.length > 0}
        <ul class="glosses">
            {#each translation.glosses as g}
                <li>
                    <span class="frag">{g.fragment}</span>
                    <span class="sep">—</span>
                    <span class="gloss">{g.gloss}</span>
                    {#if g.note && g.note.length > 0}
                        <span class="note">({g.note})</span>
                    {/if}
                </li>
            {/each}
        </ul>
    {/if}
</div>

<style>
    .lyric-line {
        padding: 12px 16px;
        transition: background-color 0.2s ease, border-color 0.2s ease;
    }
    .lyric-line.active {
        background-color: var(--selected-color);
        color: var(--text-inverted);
        border-left-color: var(--text-inverted);
    }
    .lyric-line.empty {
        padding: 4px 16px;
    }
    .original {
        font-size: 1.15em;
        font-weight: 500;
        line-height: 1.35;
    }
    .lyric-line.active .original {
        font-weight: 700;
    }
    .stanza-break {
        opacity: 0.3;
        font-size: 0.9em;
    }
    .translation {
        margin-top: 4px;
        font-size: 0.95em;
        font-style: italic;
        opacity: 0.9;
        line-height: 1.35;
    }
    .glosses {
        margin: 6px 0 0 0;
        padding: 0;
        list-style: none;
        font-size: 0.82em;
        opacity: 0.9;
        line-height: 1.5;
    }
    .glosses li {
        padding-left: 12px;
    }
    .frag {
        font-weight: 600;
    }
    .sep {
        opacity: 0.5;
        margin: 0 4px;
    }
    .note {
        margin-left: 6px;
        opacity: 0.7;
        font-style: italic;
    }
</style>
