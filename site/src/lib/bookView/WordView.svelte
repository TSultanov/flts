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
    let model = $state(defaultModel ?? 0);
    let translationRequestId: number | null = $state(null);

    let unsub: UnlistenFn | null = null;

    onDestroy(() => {
        if (unsub) {
            unsub();
        }
    });

    async function listenToTranslationRequestChanges() {
        if (translationRequestId !== null) {
            console.log(
                `Listening for translation request ${translationRequestId}`,
            );
            unsub = await listen<number>(
                "translation_request_complete",
                (cb) => {
                    if (translationRequestId === cb.payload) {
                        translationRequestId = null;
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
        translationRequestId = await library.translateParagraph(
            bookId,
            paragraphId,
            model,
            false,
        );

        await listenToTranslationRequestChanges();
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
        <div class="translate">
            <span>Translate paragraph again</span>
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
                    <div class="spin">
                        <Fa icon={faArrowsRotate} />
                    </div>
                {:else}
                    <Fa icon={faLanguage} />
                {/if}
            </button>
        </div>
    </div>
{/if}

<style>
    .container {
        display: flex;
        flex-direction: column;
        height: 100%;
    }

    .translate {
        align-content: flex-end;
        flex-grow: 1;
    }

    @keyframes spin {
        from {
            transform: rotate(0deg);
        }
        to {
            transform: rotate(360deg);
        }
    }

    .spin {
        animation: spin 2s linear infinite;
    }

    button.translate {
        width: calc(2 * var(--font-size));
        height: calc(2 * var(--font-size));
        padding: 0;
    }
</style>
