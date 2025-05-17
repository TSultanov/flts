<script lang="ts">
  import { Router, type RouteConfig } from "@mateothegreat/svelte5-router";
  import type { RouterInstance } from "@mateothegreat/svelte5-router";
  import Config from "./lib/Config.svelte";
  import Home from "./lib/Home.svelte";
  import Nav from "./lib/Nav.svelte";

  const routes: RouteConfig[] = [
    {
      name: "Home",
      component: Home,
    },
    {
      path: "config",
      name: "Config",
      component: Config,
    },
  ];

  let router = $state<RouterInstance>();
  let current = $derived(router?.current);
</script>

<Nav {routes} {current} />
<Router bind:instance={router} {routes} />

<style>
</style>
