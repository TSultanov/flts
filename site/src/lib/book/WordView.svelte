<script lang="ts">
    import { getContext, onDestroy, onMount } from "svelte";
    import type { Library } from "../data/library";
    import type { UUID } from "../data/uuid";
    import { models, configStore } from "../config/store";
    import {
        faLanguage,
        faChevronUp,
        faChevronDown,
        faBookOpen,
        faPlus,
        faCheck,
        faSpinner,
        faTriangleExclamation,
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

    // Losing word focus returns the panel to its peek bar.
    $effect(() => {
        if (!selection) expanded = false;
    });

    const wordKey = $derived(
        selection
            ? `${selection.paragraphId}:${selection.sentence}:${selection.word}`
            : null,
    );

    // Card action state is keyed by selection so an in-flight request or an
    // error never leaks onto the next word.
    let addingKey = $state<string | null>(null);
    let addedKey = $state<string | null>(null);
    let justAddedKey = $state<string | null>(null);
    let cardError = $state<{ key: string; message: string } | null>(null);
    let justAddedTimer: ReturnType<typeof setTimeout> | null = null;

    const isAddingCard = $derived(addingKey != null && addingKey === wordKey);
    const hasCard = $derived(
        word?.current?.hasCard === true ||
            (wordKey != null && addedKey === wordKey),
    );
    const showAddCard = $derived(
        word?.current?.canCreateCard === true && !hasCard,
    );
    const showAdded = $derived(
        justAddedKey != null && justAddedKey === wordKey,
    );
    const currentCardError = $derived(
        cardError != null && cardError.key === wordKey
            ? cardError.message
            : null,
    );

    async function addCard() {
        if (!selection || !word?.current || wordKey == null) return;
        const key = wordKey;
        addingKey = key;
        cardError = null;
        try {
            await library.createWordCard(
                bookId,
                selection.paragraphId,
                selection.sentence,
                selection.word,
            );
            addedKey = key;
            justAddedKey = key;
            if (justAddedTimer) clearTimeout(justAddedTimer);
            justAddedTimer = setTimeout(() => {
                if (justAddedKey === key) justAddedKey = null;
                justAddedTimer = null;
            }, 1200);
        } catch (e) {
            cardError = {
                key,
                message: e instanceof Error ? e.message : String(e),
            };
        } finally {
            if (addingKey === key) addingKey = null;
        }
    }

    onDestroy(() => {
        if (justAddedTimer) clearTimeout(justAddedTimer);
    });

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
            invoke("show_system_dictionary", {
                word: word.current.original,
            }).catch(console.error);
        }
    }

    async function translateParagraph() {
        if (selection && model) {
            await library.translateParagraph(
                bookId,
                selection.paragraphId,
                model,
                false,
            );
        }
    }
</script>

{#snippet cardAction(testId: string)}
    {#if showAddCard}
        <button
            class="peek-button"
            class:spin={isAddingCard}
            aria-label={currentCardError ?? "Add to Anki"}
            title={currentCardError ?? "Add to Anki"}
            data-testid={testId}
            disabled={isAddingCard}
            onclick={addCard}
        >
            {#if isAddingCard}
                <Fa icon={faSpinner} />
            {:else if currentCardError}
                <Fa icon={faTriangleExclamation} />
            {:else}
                <Fa icon={faPlus} />
            {/if}
        </button>
    {:else if showAdded}
        <span
            class="peek-button added"
            role="status"
            aria-label="Card added"
            data-testid={`${testId}-added`}
        >
            <Fa icon={faCheck} />
        </span>
    {/if}
{/snippet}

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
                {#if w.grammar.originalInitialForm && w.grammar.originalInitialForm !== w.original.toLowerCase()}
                    <span class="peek-original-initial-form"
                        >({w.grammar.originalInitialForm})</span
                    >
                {/if}
                {#if w.contextualTranslations && w.contextualTranslations.length > 0}
                    <span class="peek-translations"
                        >{w.contextualTranslations.join(", ")}</span
                    >
                {/if}
                {#if w.note}
                    <span class="peek-note">{w.note}</span>
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
                {@render cardAction("word-view-add-card")}
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
                {@render cardAction("word-view-add-card-expanded")}
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
                    <button
                        class="ios-dictionary-btn"
                        onclick={showSystemDictionary}
                    >
                        Show System Dictionary
                    </button>
                {/if}
                {#if currentCardError}
                    <p class="card-error" role="alert">{currentCardError}</p>
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
        white-space: nowrap;
        color: var(--dialog-text);
    }

    .peek-word {
        font-weight: 600;
    }

    .peek-original-initial-form {
        color: var(--dialog-text-secondary);
        font-style: italic;
    }

    .peek-translations {
        color: var(--dialog-text-secondary);
        overflow: hidden;
        text-overflow: ellipsis;
        min-width: 0;
    }

    .peek-note {
        color: var(--dialog-text-secondary);
        font-style: italic;
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

    .peek-button:disabled {
        cursor: not-allowed;
        opacity: 0.7;
    }

    .peek-button.added {
        cursor: default;
        color: var(--dialog-text-secondary);
    }

    .peek-button.spin :global(svg) {
        animation: card-spin 0.9s linear infinite;
    }

    @keyframes card-spin {
        from {
            transform: rotate(0deg);
        }
        to {
            transform: rotate(360deg);
        }
    }

    .peek-button :global(svg) {
        pointer-events: none;
    }

    .card-error {
        margin: 0.5em 0 0;
        color: var(--error-color);
        font-size: 0.9em;
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
