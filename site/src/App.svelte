<script lang="ts">
    import { Router, type RouteConfig } from "@mateothegreat/svelte5-router";
    import type { RouterInstance } from "@mateothegreat/svelte5-router";
    import Config from "./lib/Config.svelte";
    import Nav from "./lib/Nav.svelte";
    import TranslatorView from "./lib/TranslatorView.svelte";
    import ImportView from "./lib/ImportView.svelte";
    import { onMount, setContext } from "svelte";
    import LibraryView from "./lib/LibraryView.svelte";
    import { Library } from "./lib/library.svelte";
    import type { RouteLinkProps } from "./lib/Link.svelte";

    const routes: RouteConfig[] = [
        {
            name: "Library",
            component: LibraryView,
        },
        {
            path: "import",
            name: "Import",
            component: ImportView,
        },
        {
            path: "translator",
            name: "Translator",
            component: TranslatorView,
        },
        {
            path: "config",
            name: "Config",
            component: Config,
        },
    ];

    const links: RouteLinkProps[] = [
        {
            href: "/",
            label: "Library",
            options: {
                active: {
                    absolute: false
                }
            },
        },
        {
            href: "/import",
            label: "Import",
        },
        {
            href: "/translator",
            label: "Translator",
        },
        {
            href: "/config",
            label: "Config",
        }
    ];

    let router = $state<RouterInstance>();
    let route = $derived(router?.current);

    let nav: HTMLElement | undefined = $state();
    const mainHeight: {
        value: number
    } = $state({ value: 700 });

    setContext('mainHeight', mainHeight);

    function handleResize() {
        mainHeight.value = window.innerHeight - (nav?.clientHeight ?? 0);
    }

    let library = new Library();
    onMount(async () => {
        mainHeight.value = window.innerHeight - (nav?.clientHeight ?? 0);
        await library.loadState();
    })
    setContext('library', library);

</script>

<svelte:window onresize={handleResize} />

<div bind:this={nav}>
    <Nav {router} {route} {links} />
</div>
<div class="main" style="height: {mainHeight.value}px">
    <Router bind:instance={router} {routes} />
</div>

<style>
    .main {
        padding: 10px;
    }
</style>
