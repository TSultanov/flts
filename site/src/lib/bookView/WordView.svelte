<script lang="ts">
    import { getContext } from "svelte";
    import type { Library } from "../data/library";
    import type { UUID } from "../data/v2/db";

    const {
        bookId,
        sentenceWordIdToDisplay,
    }: { bookId: UUID; sentenceWordIdToDisplay: [number, number, number] } =
        $props();

    $inspect(sentenceWordIdToDisplay);
    const library: Library = getContext("library");
    const word = $derived(
        library.getWordInfo(
            bookId,
            sentenceWordIdToDisplay[0],
            sentenceWordIdToDisplay[1],
            sentenceWordIdToDisplay[2],
        ),
    );
</script>

{#if $word}
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
{/if}
