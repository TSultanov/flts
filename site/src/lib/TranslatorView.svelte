<script lang="ts">
    import { GoogleGenAI } from "@google/genai";
    import { getConfig } from "./config";
    import { Translator, type ParagraphTranslation } from "./data/translator";
    import { getContext, onMount } from "svelte";

    const mainHeightObj: {
        value: number;
    } = getContext("mainHeight");
    const mainHeight = $derived(mainHeightObj.value);

    let inputText = $state("");
    let output: ParagraphTranslation | null = $state(null);

    let translator: Translator | null = $state(null);

    async function translate() {
        if (translator) {
            let res = await translator.getCachedTranslation({
                paragraph: inputText,
            });

            if (res === null) {
                res = await translator.getTranslation({
                    paragraph: inputText,
                });
            }

            console.log(res);
            output = res;
        }
    }

    onMount(async () => {
        const config = await getConfig();
        const ai = new GoogleGenAI({ apiKey: config.apiKey });
        translator = await Translator.build(ai, config.targetLanguage);
    });
</script>

<div class="translation-test">
    <textarea bind:value={inputText}></textarea>
    <button onclick={translate}>Translate</button>
    <div class="output">
        <p>
            {#if output}
                {#each output.sentences as s, sIndex}
                    {#each s.words as word, wIndex}
                        {#if !word.isPunctuation}
                            {#if sIndex != 0 || wIndex != 0}
                                &#20;
                            {/if}
                            <span class="word">
                                {@html word.original}
                            </span>
                        {:else}
                            <span class="punctuation">
                                {@html word.original}
                            </span>
                        {/if}
                    {/each}
                {/each}
            {/if}
        </p>
    </div>
</div>

<style>
    .translation-test {
        display: grid;
        grid-auto-columns: auto;
        grid-auto-rows: auto auto 1fr;
        gap: 10px;
        max-width: 100%;
        margin: 0 80px 0 80px;
        height: 100%;
    }

    .translation-test textarea {
        height: 200px;
    }

    .translation-test .output {
        overflow: scroll;
        border: 1px solid #555555;
    }
</style>
