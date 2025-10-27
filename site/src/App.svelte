<script lang="ts">
    import { Router } from 'sv-router';
	import './router.ts';
    import Nav from "./lib/Nav.svelte";
    import { onMount, setContext } from "svelte";
    import { Library } from "./lib/data/library";
    import { configStore } from "./lib/config";
    import { navigate } from './router';

    $inspect($configStore);

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
            return configOnlyLinks;
        } else {
            return fullLinks;
        }
    })

    $effect(() => {
        if (!$configStore?.geminiApiKey || !$configStore?.libraryPath || !$configStore?.targetLanguageId) {
            navigate("/config");
        } else {
            navigate("/library");
        }
    });

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
    });
</script>

<svelte:window onresize={handleResize} />

<div bind:this={nav}>
    <Nav {links} />
</div>
<div class="main" style="height: {mainHeight.value}px;">
    <Router />
</div>

<style>
    .main {
        height: 100%;
    }
</style>
