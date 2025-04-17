<script>
    let { onSave } = $props();

    import { onMount } from "svelte";
    import { getConfig, setConfig } from "./config";

    let apikey = $state();

    onMount(async () => {
        let config = await getConfig();
        apikey = config?.api_key || "";
    })

    $effect(() => {
        setConfig({
            api_key: apikey
        });
    })
</script>

<label for="apikey">Gemini API KEY</label>
<input id="apikey" type="text" bind:value={apikey}>
<button onclick="{onSave}">Save</button>