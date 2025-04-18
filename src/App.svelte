<script lang="ts">
  import { onMount } from "svelte";
  import { getConfig } from "./lib/config";
  import Config from "./lib/Config.svelte";
  import Reader from "./lib/Reader.svelte";

  import type { ScreenState, ReaderState } from "./lib/screens";
    import Library from "./lib/Library.svelte";

  let state: ScreenState = $state("Library");
</script>

<main>
  {#if state === "Library"}
  <button onclick="{() => state = "Config"}">Config</button>
  {/if}
  {#if state === "Config"}
  <button onclick="{() => state = "Library"}">Library</button>
  {/if}

  {#if state === "Library"}
    <Library onBookSelect={(e) => state = e} />
  {:else if state === "Config"}
    <Config onSave={() => {state = "Library"}} />
  {:else }
    <Reader book={state.book} onClose={() => state = "Library" } />
  {/if}
</main>

<style>
</style>
