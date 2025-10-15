<script lang="ts">
    import { onMount } from "svelte";
    import { getConfig, getLanguages, getModels, setConfig, type Language, type Model } from "./config";

    
    let geminiApiKey: string = $state('');
    let targetLanguage: string = $state('');
    let model: number = $state(0);
    let models: Model[] = $state([]);

    let languages: Language[] = $state([]);
    let language = $state("rus");
    
    onMount(async () => {
        models = await getModels();
        languages = await getLanguages();
        let config = await getConfig();
    })

    async function save() {
        await setConfig({
            geminiApiKey,
            targetLanguage,
            model,
        });
    }
</script>

<div class="container">
    <div class="config-form">
        <label for="targetlanguage">Target Language</label>
        <select id="targetlanguage" bind:value={language}>
            {#each languages as language}
                <option value="{language.id}">{language.name} {language.localName ? `(${language.localName})` : ""}</option>
            {/each}
        </select>

        <label for="apikey">Gemini API KEY</label>
        <input id="apikey" type="text" bind:value={geminiApiKey}>

        <label for="model">Model</label>
        <select id="model" bind:value={model}>
            {#each models as model}
                <option value="{model.id}">{model.name}</option>
            {/each}
        </select>

        <button onclick={save} class="primary">Save</button>
    </div>
</div>

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