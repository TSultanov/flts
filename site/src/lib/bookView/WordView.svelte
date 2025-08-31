<script lang="ts">
    import { getContext } from "svelte";
    import type {
        Books,
        ParagraphTranslationSentenceWordGrammar,
    } from "../data/evolu/book";
    import type {
        BookParagraphTranslationSentenceWordId,
        DatabaseSchema,
    } from "../data/evolu/schema";
    import type { Evolu } from "@evolu/common";
    import { queryState } from "@evolu/svelte";

    const {
        sentenceWordIdToDisplay,
    }: { sentenceWordIdToDisplay: BookParagraphTranslationSentenceWordId } =
        $props();

    const books: Books = getContext("books");
    const evolu: Evolu<DatabaseSchema> = getContext("evolu");

    const wordQuery = $derived(books.wordTranslation(sentenceWordIdToDisplay));

    const wordTranslation = queryState(evolu, () => wordQuery);

    const word = $derived(wordTranslation.rows[0]);

    const wordTranslationInContext: string[] = $derived(
        word.wordTranslationInContext
            ? JSON.parse(word.wordTranslationInContext)
            : [],
    );

    const grammarContext: ParagraphTranslationSentenceWordGrammar | null =
        $derived(word.grammarContext ? JSON.parse(word.grammarContext) : null);
</script>

<p class="word-original">{@html word.original}</p>
{#if wordTranslationInContext.length > 0}
    <details open>
        <summary>Meaning</summary>
        <ul>
            {#each wordTranslationInContext as translation}
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
<!-- {#if $word.wordTranslation}
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
    {/if} -->
{#if grammarContext}
    <details>
        <summary>Grammar</summary>
        <table>
            <tbody>
                <tr>
                    <th scope="row">Part of speech</th>
                    <td>{grammarContext.partOfSpeech}</td>
                </tr>
                <tr>
                    <th scope="row">Initial form</th>
                    <td>{grammarContext.originalInitialForm}</td>
                </tr>
                {#if grammarContext.plurality}
                    <tr>
                        <th scope="row">Plurality</th>
                        <td>{grammarContext.plurality}</td>
                    </tr>
                {/if}
                {#if grammarContext.person}
                    <tr>
                        <th scope="row">Person</th>
                        <td>{grammarContext.person}</td>
                    </tr>
                {/if}
                {#if grammarContext.tense}
                    <tr>
                        <th scope="row">Tense</th>
                        <td>{grammarContext.tense}</td>
                    </tr>
                {/if}
                {#if grammarContext.case}
                    <tr>
                        <th scope="row">Case</th>
                        <td>{grammarContext.case}</td>
                    </tr>
                {/if}
                {#if grammarContext.other}
                    <tr>
                        <th scope="row">Other</th>
                        <td>{grammarContext.other}</td>
                    </tr>
                {/if}
            </tbody>
        </table>
    </details>
{/if}
{#if word.fullTranslation}
    <details>
        <summary>Full sentence</summary>
        <p>{word.fullTranslation}</p>
    </details>
{/if}
{#if word.translatingModel}
    <p>Translated by: {word.translatingModel}</p>
{/if}
