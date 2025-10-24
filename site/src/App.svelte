<script lang="ts">
    import { goto, Router, type RouteConfig } from "@mateothegreat/svelte5-router";
    import type { RouterInstance } from "@mateothegreat/svelte5-router";
    import Config from "./lib/Config.svelte";
    import Nav from "./lib/Nav.svelte";
    import ImportView from "./lib/importView/ImportView.svelte";
    import { onMount, setContext } from "svelte";
    import LibraryView from "./lib/LibraryView.svelte";
    import { Library } from "./lib/data/library";
    import BookView from "./lib/bookView/BookView.svelte";
    import { configStore } from "./lib/config";

    $inspect($configStore);

    const routes: RouteConfig[] = [
        {
            path: "",
            hooks: {
                pre: () => {
                    goto("/library");
                },
            },
        },
        {
            path: "/library",
            name: "Library",
            component: LibraryView,
        },
        {
            path: "/book/(?<bookId>[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12})(?:/(?<chapterId>[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}))?",
            name: "Book",
            component: BookView,
        },
        {
            path: "import",
            name: "Import",
            component: ImportView,
        },
        {
            path: "config",
            name: "Config",
            component: Config,
        },
    ];

    const fullLinks = [
        {
            href: "/library",
            label: "Library",
        },
        {
            href: "/import",
            label: "Import",
        },
        {
            href: "/config",
            label: "Config",
        },
    ];

    const configOnlyLinks = [
        {
            href: "/config",
            label: "Config",
        },
    ];

    const links = $derived.by(() => {
        if (!$configStore?.geminiApiKey || !$configStore?.libraryPath || !$configStore?.targetLanguageId) {
            goto("/config");
            return configOnlyLinks;
        } else {
            return fullLinks;
        }
    })

    let router = $state<RouterInstance>();

    let nav: HTMLElement | undefined = $state();
    const mainHeight: {
        value: number;
    } = $state({ value: 700 });

    setContext("mainHeight", mainHeight);

    function handleResize() {
        mainHeight.value = window.innerHeight - (nav?.clientHeight ?? 0);
    }

    const library = new Library();
    setContext("library", library);

    onMount(async () => {
        mainHeight.value = window.innerHeight - (nav?.clientHeight ?? 0);
        setContext("router", router);
    });
</script>

<svelte:window onresize={handleResize} />

<div bind:this={nav}>
    <Nav {router} {links} />
</div>
<div class="main" style="height: {mainHeight.value}px;">
    <Router bind:instance={router} {routes} />
</div>

<style>
    .main {
        height: 100%;
    }
</style>
