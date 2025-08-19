<script lang="ts">
    import { onMount } from "svelte";
    import { sqlBooks, type SentenceTranslation } from "../data/sql/book";
    import type { UUID } from "../data/v2/db";

    const { sentenceWordIdToDisplay }: { sentenceWordIdToDisplay: UUID } =
        $props();

    const wordTranslationPromise = $derived(
        sqlBooks.getWordTranslation(sentenceWordIdToDisplay),
    );

    let sentenceTranslationPromise = $derived.by(async () => {
        const word = await wordTranslationPromise;
        if (word) {
            return sqlBooks.getSentenceTranslation(word.sentenceUid);
        }
        return null;
    });
</script>

{#await wordTranslationPromise}
    <p>Loading...</p>
{:then word}
    {#if word}
        <p class="word-original">{@html word.original}</p>
        {#if word.wordTranslationInContext && word.wordTranslationInContext.length > 0}
            <details open>
                <summary>Meaning</summary>
                <ul>
                    {#each word.wordTranslationInContext as translation}
                        <li>{translation}</li>
                    {/each}
                </ul>
            </details>
        {/if}
        {#if word.note}
            <details open>
                <summary>Note</summary>
                <p>{word.note}</p>
            </details>
        {/if}
        <!-- TODO -->
        <!-- {#if word.wordTranslation}
        <details>
            <summary>Dictionary</summary>
            <table>
                <tbody>
                    <tr>
                        <th scope="row">Language</th>
                        <td
                            >{word.wordTranslation.originalWord.originalLanguage
                                .name}</td
                        >
                    </tr>
                    <tr>
                        <th scope="row">Translation</th>
                        <td>{word.wordTranslation.translation}</td>
                    </tr>
                </tbody>
            </table>
        </details>
    {/if} -->
        {#if word.grammarContext}
            <details>
                <summary>Grammar</summary>
                <table>
                    <tbody>
                        <tr>
                            <th scope="row">Part of speech</th>
                            <td>{word.grammarContext.partOfSpeech}</td>
                        </tr>
                        {#if word.grammarContext.originalInitialForm}
                            <tr>
                                <th scope="row">Initial form</th>
                                <td
                                    >{word.grammarContext
                                        .originalInitialForm}</td
                                >
                            </tr>
                        {/if}
                        {#if word.grammarContext.plurality}
                            <tr>
                                <th scope="row">Plurality</th>
                                <td>{word.grammarContext.plurality}</td>
                            </tr>
                        {/if}
                        {#if word.grammarContext.person}
                            <tr>
                                <th scope="row">Person</th>
                                <td>{word.grammarContext.person}</td>
                            </tr>
                        {/if}
                        {#if word.grammarContext.tense}
                            <tr>
                                <th scope="row">Tense</th>
                                <td>{word.grammarContext.tense}</td>
                            </tr>
                        {/if}
                        {#if word.grammarContext.case}
                            <tr>
                                <th scope="row">Case</th>
                                <td>{word.grammarContext.case}</td>
                            </tr>
                        {/if}
                        {#if word.grammarContext.other}
                            <tr>
                                <th scope="row">Other</th>
                                <td>{word.grammarContext.other}</td>
                            </tr>
                        {/if}
                    </tbody>
                </table>
            </details>
        {/if}
        {#if sentenceTranslationPromise}
            {#await sentenceTranslationPromise}
                <p>Loading...</p>
            {:then sentence}
                {#if sentence}
                    <details>
                        <summary>Full sentence</summary>
                        <p>{sentence.fullTranslation}</p>
                    </details>
                    <p>Translated by: {sentence.translatingModel}</p>
                {/if}
            {/await}
        {/if}
    {/if}
{/await}
