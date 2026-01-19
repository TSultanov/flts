<script lang="ts">
    import { getContext, onDestroy, onMount } from "svelte";
    import type { Library, TranslationStatus } from "../data/library";
    import type { UUID } from "../data/v2/db";
    import { models, configStore } from "../config";
    import { faLanguage } from "@fortawesome/free-solid-svg-icons";
    import { listen, type UnlistenFn } from "@tauri-apps/api/event";
    import Fa from "svelte-fa";
    import CircularProgress from "../widgets/CircularProgress.svelte";
    import { invoke } from "@tauri-apps/api/core";

    const {
        bookId,
        sentenceWordIdToDisplay,
    }: { bookId: UUID; sentenceWordIdToDisplay: [number, number, number] } =
        $props();

    const paragraphId = $derived(sentenceWordIdToDisplay[0]);
    const library: Library = getContext("library");
    const word = $derived(
        library.getWordInfo(
            bookId,
            paragraphId,
            sentenceWordIdToDisplay[1],
            sentenceWordIdToDisplay[2],
        ),
    );

    let defaultModel = $configStore?.model;
    let model = $derived($word?.translationModel);
    let translationRequestId: number | null = $state(null);

    let progressChars = $state(0);
    let expectedChars = $state(100);

    let unsub: UnlistenFn | null = null;

    listen<TranslationStatus>("translation_status", (event) => {
        const status = event.payload;
        if (status.request_id === translationRequestId) {
            if (status.is_complete) {
                translationRequestId = null;
                progressChars = 0;
            } else {
                progressChars = status.progress_chars;
                expectedChars = status.expected_chars;
            }
        }
    }).then((u) => {
        unsub = u;
    });

    // System Dictionary (macOS) - using library method which returns a Readable store
    const systemDefinition = $derived(
        $word?.original
            ? library.getSystemDefinition(
                  $word.original,
                  $word.sourceLanguage || "en",
                  $configStore?.targetLanguageId || "en",
              )
            : null,
    );

    // Detect iOS platform - initialized in onMount
    let isIos = $state(false);

    onMount(() => {
        // Lazy import to avoid module-level issues
        import("@tauri-apps/plugin-os")
            .then(({ platform }) => {
                try {
                    const p = platform();
                    // Handle both sync result and potential promise
                    if (p && typeof (p as any).then === "function") {
                        (p as unknown as Promise<string>)
                            .then((val) => {
                                isIos = val === "ios";
                            })
                            .catch(() => {
                                isIos = false;
                            });
                    } else {
                        isIos = p === "ios";
                    }
                } catch {
                    isIos = false;
                }
            })
            .catch(() => {
                // Module not available
                isIos = false;
            });
    });

    function showSystemDictionary() {
        if ($word?.original) {
            invoke("show_system_dictionary", { word: $word.original }).catch(
                console.error,
            );
        }
    }

    onDestroy(() => {
        if (unsub) {
            unsub();
        }
    });

    const isTranslating = $derived(translationRequestId !== null);

    $effect(() => {
        if (translationRequestId === null) {
            return;
        }

        let cancelled = false;
        const interval = setInterval(async () => {
            if (cancelled) {
                return;
            }
            try {
                const id = await library.getParagraphTranslationRequestId(
                    bookId,
                    paragraphId,
                );
                if (cancelled) {
                    return;
                }
                if (id === null) {
                    translationRequestId = null;
                    progressChars = 0;
                }
            } catch {
            }
        }, 1000);

        return () => {
            cancelled = true;
            clearInterval(interval);
        };
    });

    $effect(() => {
        library
            .getParagraphTranslationRequestId(bookId, paragraphId)
            .then((id) => {
                translationRequestId = id;
                if (id !== null) {
                    progressChars = 0;
                }
            });
    });

    async function translateParagraph() {
        if (model !== 0) {
            progressChars = 0;
            translationRequestId = await library.translateParagraph(
                bookId,
                paragraphId,
                model,
                false,
            );
        }
    }
</script>

{#if $word}
    <div class="container">
        <p class="word-original">{@html $word.original}</p>
        {#if $word.contextualTranslations && $word.contextualTranslations.length > 0}
            <details open>
                <summary>Meaning</summary>
                <ul>
                    {#each $word.contextualTranslations as translation}
                        <li>{translation}</li>
                    {/each}
                </ul>
            </details>
        {/if}
        {#if $word.note}
            <details open>
                <summary>Note</summary>
                <p>{$word.note}</p>
            </details>
        {/if}
        <!-- TODO -->
        <!-- {#if $word.wordTranslation}
        <details>
            <summary>Dictionary</summary>
            <table>
                <tbody>
                    <tr>
                        <th scope="row">Language</th>
                        <td
                            >{$word.wordTranslation.originalWord
                                .originalLanguage.name}</td
                        >
                    </tr>
                    <tr>
                        <th scope="row">Translation</th>
                        <td>{$word.wordTranslation.translation}</td>
                    </tr>
                </tbody>
            </table>
        </details>
    {/if} -->
        {#if $word.grammar}
            <details>
                <summary>Grammar</summary>
                <table>
                    <tbody>
                        <tr>
                            <th scope="row">Part of speech</th>
                            <td>{$word.grammar.partOfSpeech}</td>
                        </tr>
                        {#if $word.grammar.originalInitialForm}
                            <tr>
                                <th scope="row">Initial form</th>
                                <td>{$word.grammar.originalInitialForm}</td>
                            </tr>
                        {/if}
                        {#if $word.grammar.plurality}
                            <tr>
                                <th scope="row">Plurality</th>
                                <td>{$word.grammar.plurality}</td>
                            </tr>
                        {/if}
                        {#if $word.grammar.person}
                            <tr>
                                <th scope="row">Person</th>
                                <td>{$word.grammar.person}</td>
                            </tr>
                        {/if}
                        {#if $word.grammar.tense}
                            <tr>
                                <th scope="row">Tense</th>
                                <td>{$word.grammar.tense}</td>
                            </tr>
                        {/if}
                        {#if $word.grammar.case}
                            <tr>
                                <th scope="row">Case</th>
                                <td>{$word.grammar.case}</td>
                            </tr>
                        {/if}
                        {#if $word.grammar.other}
                            <tr>
                                <th scope="row">Other</th>
                                <td>{$word.grammar.other}</td>
                            </tr>
                        {/if}
                    </tbody>
                </table>
            </details>
            <details>
                <summary>Full sentence</summary>
                <p>{$word.fullSentenceTranslation}</p>
            </details>
        {/if}
        {#if systemDefinition && $systemDefinition}
            <details open>
                <summary>System Dictionary</summary>
                <div class="definition">
                    {@html $systemDefinition.definition}
                </div>
            </details>
        {/if}
        {#if isIos}
            <button class="ios-dictionary-btn" onclick={showSystemDictionary}>
                Show System Dictionary
            </button>
        {/if}
        <div class="translate-section">
            <span>Translate paragraph again</span>
            <div class="controls">
                <select id="model" bind:value={model}>
                    {#each $models as model}
                        <option value={model.id}>{model.name}</option>
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

<style>
    .container {
        display: flex;
        flex-direction: column;
        height: 100%;
    }

    .translate-section {
        margin-top: auto;
        padding-top: 1em;
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
