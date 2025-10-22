<script lang="ts">
    import { onMount } from "svelte";
    import {
        getConfig,
        getLanguages,
        getModels,
        setConfig,
        type Model,
    } from "./config";

    let geminiApiKey: string | undefined = $state(undefined);
    let targetLanguage: string | undefined = $state("rus");
    let libraryPath: string | undefined = $state(undefined);
    let model: number = $state(0);
    let models: Model[] = $state([]);

    let languages = getLanguages();

    onMount(async () => {
        models = await getModels();
        let config = await getConfig();
        geminiApiKey = config.geminiApiKey;
        targetLanguage = config.targetLanguageId;
        libraryPath = config.libraryPath;
        model = config.model;
    });

    async function save() {
        await setConfig({
            geminiApiKey,
            targetLanguageId: targetLanguage,
            model,
            libraryPath,
        });
    }
</script>

{#await languages}
    Loading...
{:then languages}
    <div class="container">
        <div class="config-form">
            <label for="targetlanguage">Target Language</label>
            <select id="targetlanguage" bind:value={targetLanguage}>
                {#each languages as language}
                    <option value={language.id}
                        >{language.name}
                        {language.localName
                            ? `(${language.localName})`
                            : ""}</option
                    >
                {/each}
            </select>

            <label for="apikey">Gemini API KEY</label>
            <input id="apikey" type="text" bind:value={geminiApiKey} />

            <label for="model">Model</label>
            <select id="model" bind:value={model}>
                {#each models as model}
                    <option value={model.id}>{model.name}</option>
                {/each}
            </select>

            <button onclick={save} class="primary">Save</button>
        </div>
    </div>
{/await}

<style>
    .container {
        display: flex;
        justify-content: center;
        align-items: center;
        height: 100%;
    }

    .config-form {
        max-width: 500px;
        display: grid;
        gap: 10px;
        grid-auto-columns: repeat(3, 1fr) auto;
    }

    button {
        grid-column: 1/3;
    }
</style>
