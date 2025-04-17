<script lang="ts">
    import { onMount } from 'svelte';
  import { getConfig } from './lib/config';
  import Config from './lib/Config.svelte';
  import Reader from './lib/Reader.svelte'

  let showReader = $state(false);

  onMount(async () => {
    let config = await getConfig();
    showReader = config?.api_key !== null;
  })
</script>

<main>
  {#if showReader}
    <Reader />
  {:else}
    <Config onSave={() => showReader = true}/>
  {/if}
</main>

<style>
</style>
