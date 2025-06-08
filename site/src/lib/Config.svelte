<script lang="ts">
    import { onMount } from "svelte";
    import { getConfig, setConfig, type Config } from "./config";
    import { models, type ModelId } from "./data/translators/translator";

    let geminiApiKey: string = $state('');
    let targetLanguage: string = $state('');
    let model: ModelId = $state("gemini-2.5-flash-preview-05-20");

    onMount(async () => {
        let config = await getConfig();
        geminiApiKey = config?.geminiApiKey ?? '';
        targetLanguage = config?.targetLanguage ?? '';
        model = config?.model;
    })

    async function save() {
        await setConfig({
            geminiApiKey: geminiApiKey,
            targetLanguage,
            model,
        });
    }
</script>

<div class="container">
    <div class="config-form">
        <label for="targetlanguage">Target Language</label>
        <input id="targetlanguage" type="text" bind:value={targetLanguage}>

        <label for="apikey">Gemini API KEY</label>
        <input id="apikey" type="text" bind:value={geminiApiKey}>

        <label for="model">Model</label>
        <select id="model" bind:value={model}>
            {#each models as model}
                <option value="{model.id}">{model.name}</option>
            {/each}
        </select>

        <button onclick={save}>Save</button>
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