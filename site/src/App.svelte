<script lang="ts">
    import { Router, type RouteConfig } from "@mateothegreat/svelte5-router";
    import type { RouterInstance } from "@mateothegreat/svelte5-router";
    import Config from "./lib/Config.svelte";
    import Nav from "./lib/Nav.svelte";
    import TranslatorView from "./lib/TranslatorView.svelte";
    import { onMount, setContext } from "svelte";

    const routes: RouteConfig[] = [
        {
            name: "Translator",
            component: TranslatorView,
        },
        {
            path: "config",
            name: "Config",
            component: Config,
        },
    ];

    let router = $state<RouterInstance>();
    let current = $derived(router?.current);

    let nav: HTMLElement | undefined = $state();
    const mainHeight: {
        value: number
    } = $state({ value: 700 });

    setContext('mainHeight', mainHeight);

    function handleResize() {
        mainHeight.value = window.innerHeight - (nav?.clientHeight ?? 0) - 20;
    }

    onMount(async () => {
        mainHeight.value = window.innerHeight - (nav?.clientHeight ?? 0) - 20;
    })
</script>

<svelte:window onresize={handleResize} />

<div bind:this={nav}>
    <Nav {routes} {current} />
</div>
<div class="main">
    <Router bind:instance={router} {routes} />
</div>

<style>
    .main {
        padding: 10px;
        height: 100%;
    }
</style>
