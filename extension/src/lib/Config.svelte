<script>
    import { onMount } from "svelte";
    import { getConfig, setConfig } from "./config";

    let apikey = $state();
    let to = $state();

    onMount(async () => {
        let config = await getConfig();
        apikey = config?.apiKey || "";
        to = config?.to || "";
    })

    $effect(() => {
        setConfig({
            apiKey: apikey,
            to: to
        });
    })
</script>

<div>
    <label for="apikey">Gemini API KEY</label>
    <input id="apikey" type="text" bind:value={apikey}>

    <label for="to">Target Language</label>
    <input id="to" type="text" bind:value={to}>
</div>