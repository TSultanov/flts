<script lang="ts">
    import { getContext } from "svelte";
    import { type Library } from "../library.svelte";

    const { wordId }: { wordId: number } = $props();

    const library: Library = getContext("library");

    const word = $derived(library.getWordTranslation(wordId));
</script>

{#if $word}
    <p class="word-original">{@html $word.original}</p>
    {#if $word.wordTranslationInContext && $word.wordTranslationInContext.length > 0}
        <details open>
            <summary>Meaning</summary>
            <ul>
                {#each $word.wordTranslationInContext as translation}
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
    {#if $word.wordTranslation}
        <details>
            <summary>Dictionary</summary>
            <table>
                <tbody>
                    <tr>
                        <th scope="row">Language</th>
                        <td
                            >{$word.wordTranslation.originalWord.originalLanguage
                                .name}</td
                        >
                    </tr>
                    <tr>
                        <th scope="row">Translation</th>
                        <td>{$word.wordTranslation.translation}</td>
                    </tr>
                </tbody>
            </table>
        </details>
    {/if}
    {#if $word.grammarContext}
        <details>
            <summary>Grammar</summary>
            <table>
                <tbody>
                    <tr>
                        <th scope="row">Part of speech</th>
                        <td>{$word.grammarContext.partOfSpeech}</td>
                    </tr>
                    {#if $word.grammarContext.plurality}
                        <tr>
                            <th scope="row">Plurality</th>
                            <td>{$word.grammarContext.plurality}</td>
                        </tr>
                    {/if}
                    {#if $word.grammarContext.person}
                        <tr>
                            <th scope="row">Person</th>
                            <td>{$word.grammarContext.person}</td>
                        </tr>
                    {/if}
                    {#if $word.grammarContext.tense}
                        <tr>
                            <th scope="row">Tense</th>
                            <td>{$word.grammarContext.tense}</td>
                        </tr>
                    {/if}
                    {#if $word.grammarContext.case}
                        <tr>
                            <th scope="row">Case</th>
                            <td>{$word.grammarContext.case}</td>
                        </tr>
                    {/if}
                    {#if $word.grammarContext.other}
                        <tr>
                            <th scope="row">Other</th>
                            <td>{$word.grammarContext.other}</td>
                        </tr>
                    {/if}
                </tbody>
            </table>
        </details>
    {/if}
    <details>
        <summary>Full sentence</summary>
        <p>{$word.fullSentenceTranslation}</p>
    </details>
    <p>Translated by: {$word.model}</p>
{/if}
