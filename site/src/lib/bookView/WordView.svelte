<script lang="ts">
    import { getContext, onMount } from "svelte";
    import type { Library, TranslationStatus } from "../data/library";
    import type { UUID } from "../data/v2/db";
    import { models, configStore } from "../config";
    import { faLanguage } from "@fortawesome/free-solid-svg-icons";
    import Fa from "svelte-fa";
    import CircularProgress from "../widgets/CircularProgress.svelte";
    import { invoke } from "@tauri-apps/api/core";
    import { platform } from "@tauri-apps/plugin-os";

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

    let model = $derived(word.current?.translationModel);
    let translationRequestId: number | null = $state(null);

    let progressChars = $state(0);
    let expectedChars = $state(100);
    const translationStatus = $derived(
        translationRequestId !== null
            ? library.getTranslationStatus(translationRequestId)
            : null,
    );

    $effect(() => {
        const status: TranslationStatus | undefined = translationStatus?.current;
        if (!status) {
            return;
        }
        if (status.is_complete) {
            if (status.error) {
                console.warn(`Translation failed for paragraph ${paragraphId}:`, status.error);
            }
            translationRequestId = null;
            progressChars = 0;
            return;
        }

        progressChars = status.progress_chars;
        expectedChars = status.expected_chars;
    });

    const systemDefinition = $derived(
        word.current?.original
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
        if (word.current?.original) {
            invoke("show_system_dictionary", { word: word.current.original }).catch(
                console.error,
            );
        }
    }

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

{#if word.current}
    {@const w = word.current}
    <div class="container">
        <p class="word-original">{@html w.original}</p>
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
            <details open>
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
        <div class="translate-section">
            <span>Translate paragraph again</span>
            <div class="controls">
                <select id="model" bind:value={model}>
                    {#each models.current ?? [] as model}
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
