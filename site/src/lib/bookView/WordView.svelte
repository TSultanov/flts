<script lang="ts">
    import { getContext } from "svelte";
    import type { Library } from "../library.svelte";

    const { wordId }: { wordId: number | null } = $props();

    const library: Library = getContext("library");

    const word = $derived.by(() => {
        if (wordId) {
            return library.getWordTranslation(wordId);
        }

        return null;
    });
</script>

{#await word}
    <p>Loading...</p>
{:then word}
    {#if word}
        <h1>Word</h1>
        <p>{word?.original}</p>
        {#if word.wordTranslationInContext}
            <h1>Meaning</h1>
            <ul>
                {#each word.wordTranslationInContext as translation}
                    <li>{translation}</li>
                {/each}
            </ul>
        {/if}
        {#if word.note}
            <h1>Note</h1>
            <p>{word.note}</p>
        {/if}
        {#if word.wordTranslation}
            <h1>Dictionary</h1>
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
        {/if}
        {#if word.grammarContext}
            <h1>Grammar</h1>
            <table>
                <tbody>
                    <tr>
                        <th scope="row">Part of speech</th>
                        <td>{word.grammarContext.partOfSpeech}</td>
                    </tr>
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
        {/if}
    {/if}
{:catch}
    <p>Failed to load word {wordId}</p>
{/await}
