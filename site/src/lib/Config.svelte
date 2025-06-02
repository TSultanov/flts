<script lang="ts">
    import { onMount } from "svelte";
    import { getConfig, setConfig, type Config } from "./config";

    let apiKey: string = $state('');
    let targetLanguage: string = $state('');
    let model: string = $state('');

    onMount(async () => {
        let config = await getConfig();
        apiKey = config?.apiKey ?? '';
        targetLanguage = config?.targetLanguage ?? '';
        model = config?.model;
    })

    async function save() {
        await setConfig({
            apiKey,
            targetLanguage,
            model,
        });
    }
</script>

<div class="container">
    <div class="config-form">
        <label for="apikey">Gemini API KEY</label>
        <input id="apikey" type="text" bind:value={apiKey}>

        <label for="targetlanguage">Target Language</label>
        <input id="targetlanguage" type="text" bind:value={targetLanguage}>

        <label for="model">Model</label>
        <select id="model" bind:value={model}>
            <option value="gemini-2.5-flash-preview-05-20">gemini-2.5-flash-preview-05-20</option>
            <option value="gemini-2.5-pro-preview-05-06">gemini-2.5-pro-preview-05-06</option>
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