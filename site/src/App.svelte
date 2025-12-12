<script lang="ts">
    import { Router } from 'sv-router';
	import './router.ts';
    import Nav from "./lib/Nav.svelte";
    import { onMount, setContext } from "svelte";
    import { Library } from "./lib/data/library";
    import { configStore } from "./lib/config";
    import { navigate } from './router';

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
        const apiKeyOk = $configStore?.translationProvider === 'openai'
            ? !!$configStore?.openaiApiKey
            : !!$configStore?.geminiApiKey;

        if (!apiKeyOk || !$configStore?.libraryPath || !$configStore?.targetLanguageId) {
            return configOnlyLinks;
        } else {
            return fullLinks;
        }
    })

    // Only redirect if on root path, otherwise respect the current URL
    let initialRedirectDone = false;
    $effect(() => {
        if (initialRedirectDone) return;
        if ($configStore === undefined) return; // Wait for config to load

        initialRedirectDone = true;
        const currentPath = window.location.pathname;

        // Only redirect from root or if config is incomplete
        const apiKeyOk = $configStore?.translationProvider === 'openai'
            ? !!$configStore?.openaiApiKey
            : !!$configStore?.geminiApiKey;
        const configComplete = apiKeyOk && $configStore?.libraryPath && $configStore?.targetLanguageId;

        if (!configComplete) {
            // Must go to config if not configured
            if (currentPath !== '/config') {
                navigate("/config");
            }
        } else if (currentPath === '/' || currentPath === '') {
            // Only redirect from root to library
            navigate("/library");
        }
        // Otherwise, stay on the current page
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
