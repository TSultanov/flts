<script lang="ts">
    import { onMount } from "svelte";
    import { getConfig, setConfig, type Config } from "./config";

    let apiKey: string = $state('');
    let targetLanguage: string = $state('');

    onMount(async () => {
        let config = await getConfig();
        apiKey = config?.apiKey ?? '';
        targetLanguage = config?.targetLanguage ?? '';
    })

    async function save() {
        await setConfig({
            apiKey,
            targetLanguage
        });
    }
</script>

<div class="container">
    <div class="config-form">
        <label for="apikey">Gemini API KEY</label>
        <input id="apikey" type="text" bind:value={apiKey}>

        <label for="targetlanguage">Target Language</label>
        <input id="targetlanguage" type="text" bind:value={targetLanguage}>

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
        max-width: 300px;
        display: grid;
        gap: 10px;
        grid-template-columns: auto auto;
    }

    button {
        grid-column: 1/3;
    }
</style>