<script lang="ts">
    import { getContext, onDestroy } from "svelte";
    import type { Library } from "../data/library";
    import type { UUID } from "../data/v2/db";
    import { models, configStore } from "../config";
    import {
        faLanguage,
        faArrowsRotate,
    } from "@fortawesome/free-solid-svg-icons";
    import { listen, type UnlistenFn } from "@tauri-apps/api/event";
    import Fa from "svelte-fa";
    import CircularProgress from "../widgets/CircularProgress.svelte";

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

    let unsub: UnlistenFn | null = null;
    let unsubProgress: UnlistenFn | null = null;

    // Progress tracking
    let progressChars = $state(0);
    let expectedChars = $state(100);

    onDestroy(() => {
        if (unsub) {
            unsub();
        }
        if (unsubProgress) {
            unsubProgress();
        }
    });

    async function listenToTranslationRequestChanges() {
        if (translationRequestId !== null) {
            console.log(
                `Listening for translation request ${translationRequestId}`,
            );

            // Reset progress
            progressChars = 0;

            unsub = await listen<number>(
                "translation_request_complete",
                (cb) => {
                    if (translationRequestId === cb.payload) {
                        translationRequestId = null;
                        if (unsubProgress) {
                            unsubProgress();
                            unsubProgress = null;
                        }
                    }
                },
            );

            unsubProgress = await listen<[number, string, number]>(
                "translation_progress",
                (cb) => {
                    const [reqId, chunk, total] = cb.payload;
                    if (reqId === translationRequestId) {
                        // chunk is the accumulated translation so far, not a delta
                        progressChars = chunk.length;
                        expectedChars = total;
                    }
                },
            );
        }
    }

    $effect(() => {
        library
            .getParagraphTranslationRequestId(bookId, paragraphId)
            .then((id) => {
                translationRequestId = id;
                listenToTranslationRequestChanges();
            });
    });

    async function translateParagraph() {
        if (model !== 0) {
            translationRequestId = await library.translateParagraph(
                bookId,
                paragraphId,
                model,
                false,
            );

            await listenToTranslationRequestChanges();
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
                    disabled={translationRequestId !== null}
                >
                    {#if translationRequestId !== null}
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
</style>
