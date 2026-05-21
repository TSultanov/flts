<script lang="ts">
    import { getContext, onMount } from "svelte";
    import type { Library } from "../data/library";
    import type { UUID } from "../data/uuid";
    import { models, configStore } from "../config/store";
    import {
        faLanguage,
        faChevronUp,
        faChevronDown,
        faBookOpen,
    } from "@fortawesome/free-solid-svg-icons";
    import Fa from "svelte-fa";
    import CircularProgress from "../widgets/CircularProgress.svelte";
    import ResizableOverlayPanel from "../widgets/ResizableOverlayPanel.svelte";
    import { invoke } from "@tauri-apps/api/core";
    import { platform } from "@tauri-apps/plugin-os";
    import type { WordSelection } from "./ParagraphViewModel.svelte";

    const {
        bookId,
        selection,
    }: { bookId: UUID; selection: WordSelection | null } = $props();

    let expanded = $state(false);
    let height = $state(320);

    const library: Library = getContext("library");
    const word = $derived(
        selection
            ? library.getWordInfo(
                  bookId,
                  selection.paragraphId,
                  selection.sentence,
                  selection.word,
              )
            : null,
    );

    let model = $derived(word?.current?.translationModel);

    const activity = $derived(
        selection
            ? library.getParagraphTranslationActivity(
                  bookId,
                  selection.paragraphId,
              )
            : null,
    );
    const isTranslating = $derived(activity?.current != null);
    const progressChars = $derived(activity?.current?.progressChars ?? 0);
    const expectedChars = $derived(activity?.current?.expectedChars ?? 100);

    const systemDefinition = $derived(
        word?.current?.original
            ? library.getSystemDefinition(
                  word.current.original,
                  word.current.sourceLanguage || "en",
                  configStore.current?.targetLanguageId || "en",
              )
            : null,
    );

    let isIos = $state(false);

    onMount(() => {
        try {
            isIos = platform() === "ios";
        } catch {
            isIos = false;
        }
    });

    function showSystemDictionary() {
        if (word?.current?.original) {
            invoke("show_system_dictionary", { word: word.current.original }).catch(
                console.error,
            );
        }
    }

    async function translateParagraph() {
        if (selection && model !== undefined && model !== 0) {
            await library.translateParagraph(
                bookId,
                selection.paragraphId,
                model,
                false,
            );
        }
    }
</script>

<ResizableOverlayPanel
    side="bottom"
    bind:expanded
    bind:size={height}
    minSize={200}
    maxSizeRatio={0.7}
    collapsedSize={48}
    shortcut="w"
    testId="word-view"
>
    {#snippet peek()}
        <div class="peek" data-testid="word-view-peek">
            {#if word?.current}
                {@const w = word.current}
                <span class="peek-word">{@html w.original}</span>
                {#if w.contextualTranslations && w.contextualTranslations.length > 0}
                    <span class="peek-translations"
                        >{w.contextualTranslations.join(", ")}</span
                    >
                {/if}
                <div class="peek-spacer"></div>
                {#if isIos}
                    <button
                        class="peek-button"
                        aria-label="Show system dictionary"
                        onclick={showSystemDictionary}
                    >
                        <Fa icon={faBookOpen} />
                    </button>
                {/if}
                <button
                    class="peek-button"
                    aria-label="Expand word details"
                    data-testid="word-view-expand"
                    onclick={() => (expanded = true)}
                >
                    <Fa icon={faChevronUp} />
                </button>
            {:else}
                <span class="peek-hint">Select a word to show translation</span>
            {/if}
        </div>
    {/snippet}

    {#if word?.current}
        {@const w = word.current}
        <div class="expanded-body" data-testid="word-view-expanded">
            <header class="expanded-header">
                <p class="word-original">{@html w.original}</p>
                <button
                    class="collapse-button"
                    aria-label="Collapse word details"
                    data-testid="word-view-collapse"
                    onclick={() => (expanded = false)}
                >
                    <Fa icon={faChevronDown} />
                </button>
            </header>
            <div class="expanded-scroll">
                {#if w.contextualTranslations && w.contextualTranslations.length > 0}
                    <details open>
                        <summary>Meaning</summary>
                        <ul>
                            {#each w.contextualTranslations as translation}
                                <li>{translation}</li>
                            {/each}
                        </ul>
                    </details>
                {/if}
                {#if w.note}
                    <details open>
                        <summary>Note</summary>
                        <p>{w.note}</p>
                    </details>
                {/if}
                {#if w.grammar}
                    <details>
                        <summary>Grammar</summary>
                        <table>
                            <tbody>
                                <tr>
                                    <th scope="row">Part of speech</th>
                                    <td>{w.grammar.partOfSpeech}</td>
                                </tr>
                                {#if w.grammar.originalInitialForm}
                                    <tr>
                                        <th scope="row">Initial form</th>
                                        <td>{w.grammar.originalInitialForm}</td>
                                    </tr>
                                {/if}
                                {#if w.grammar.plurality}
                                    <tr>
                                        <th scope="row">Plurality</th>
                                        <td>{w.grammar.plurality}</td>
                                    </tr>
                                {/if}
                                {#if w.grammar.person}
                                    <tr>
                                        <th scope="row">Person</th>
                                        <td>{w.grammar.person}</td>
                                    </tr>
                                {/if}
                                {#if w.grammar.tense}
                                    <tr>
                                        <th scope="row">Tense</th>
                                        <td>{w.grammar.tense}</td>
                                    </tr>
                                {/if}
                                {#if w.grammar.case}
                                    <tr>
                                        <th scope="row">Case</th>
                                        <td>{w.grammar.case}</td>
                                    </tr>
                                {/if}
                                {#if w.grammar.other}
                                    <tr>
                                        <th scope="row">Other</th>
                                        <td>{w.grammar.other}</td>
                                    </tr>
                                {/if}
                            </tbody>
                        </table>
                    </details>
                    <details>
                        <summary>Full sentence</summary>
                        <p>{w.fullSentenceTranslation}</p>
                    </details>
                {/if}
                {#if systemDefinition?.current}
                    <details>
                        <summary>System Dictionary</summary>
                        <div class="definition">
                            {@html systemDefinition.current.definition}
                        </div>
                    </details>
                {/if}
                {#if isIos}
                    <button class="ios-dictionary-btn" onclick={showSystemDictionary}>
                        Show System Dictionary
                    </button>
                {/if}
            </div>
            <div class="translate-section">
                <span>Translate paragraph again</span>
                <div class="controls">
                    <select id="model" bind:value={model}>
                        {#each models.current ?? [] as m}
                            <option value={m.id}>{m.name}</option>
                        {/each}
                    </select>
                    <button
                        class="translate"
                        aria-label="Translate paragraph again"
                        onclick={translateParagraph}
                        disabled={isTranslating}
                    >
                        {#if isTranslating}
                            <CircularProgress
                                value={progressChars}
                                max={expectedChars}
                                size="1.2em"
                                strokeWidth={4}
                            />
                        {:else}
                            <Fa icon={faLanguage} />
                        {/if}
                    </button>
                </div>
            </div>
        </div>
    {/if}
</ResizableOverlayPanel>

<style>
    .peek {
        flex: 1 1 auto;
        display: flex;
        align-items: center;
        gap: 0.6em;
        padding: 0 12px;
        height: 100%;
        overflow: hidden;
        white-space: nowrap;
        color: var(--dialog-text);
    }

    .peek-word {
        font-weight: 600;
    }

    .peek-translations {
        color: var(--dialog-text-secondary);
        overflow: hidden;
        text-overflow: ellipsis;
        min-width: 0;
    }

    .peek-hint {
        color: var(--dialog-text-secondary);
    }

    .peek-spacer {
        flex: 1 1 auto;
    }

    .peek-button {
        flex: 0 0 auto;
        width: 32px;
        height: 32px;
        padding: 0;
        border: 1px solid var(--dialog-border);
        border-radius: 4px;
        background-color: var(--dialog-background);
        color: var(--dialog-text);
        display: flex;
        align-items: center;
        justify-content: center;
        cursor: pointer;
    }

    .peek-button:hover:not(:disabled) {
        background-color: var(--button-cancel-hover);
    }

    .peek-button :global(svg) {
        pointer-events: none;
    }

    .expanded-body {
        flex: 1 1 auto;
        display: flex;
        flex-direction: column;
        min-height: 0;
        padding: 10px 16px 10px 16px;
        color: var(--dialog-text);
    }

    .expanded-header {
        display: flex;
        align-items: center;
        gap: 0.6em;
        margin-bottom: 0.3em;
    }

    .word-original {
        flex: 1 1 auto;
        font-weight: 600;
        margin: 0;
    }

    .collapse-button {
        flex: 0 0 auto;
        width: 32px;
        height: 32px;
        padding: 0;
        border: 1px solid var(--dialog-border);
        border-radius: 4px;
        background-color: var(--dialog-background);
        color: var(--dialog-text);
        display: flex;
        align-items: center;
        justify-content: center;
        cursor: pointer;
    }

    .collapse-button :global(svg) {
        pointer-events: none;
    }

    .expanded-scroll {
        flex: 1 1 auto;
        min-height: 0;
        overflow-y: auto;
    }

    .translate-section {
        margin-top: 0.5em;
        padding-top: 0.5em;
        border-top: 1px solid var(--dialog-border);
        display: flex;
        flex-direction: column;
        gap: 0.5em;
    }

    .controls {
        display: flex;
        gap: 0.5em;
        align-items: center;
    }

    select {
        flex-grow: 1;
        width: 100%;
    }

    button.translate {
        flex-shrink: 0;
        width: calc(2 * var(--font-size));
        height: calc(2 * var(--font-size));
        padding: 0;
        display: flex;
        align-items: center;
        justify-content: center;
    }

    .definition {
        margin: 0.5em 0;
    }

    :global(.definition .exg),
    :global(.definition .semb),
    :global(.definition .hwg),
    :global(.definition .gramb) {
        display: block;
        margin: 0.5em 0;
    }

    :global(.definition .exg) {
        margin: 0.5em 0.5em;
    }
</style>
