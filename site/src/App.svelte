<script lang="ts">
    import { Router } from 'sv-router';
	import './router';
    import Nav from "./lib/chrome/Nav.svelte";
    import AnkiSyncButton from "./lib/chrome/AnkiSyncButton.svelte";
    import SyncStatusButton from "./lib/sync/SyncStatusButton.svelte";
    import { onMount, setContext } from "svelte";
    import { Library } from "./lib/data/library";
    import { configStore } from "./lib/config/store";
    import { navigate } from './router';
    import { platform } from '@tauri-apps/plugin-os';
    import { invoke } from '@tauri-apps/api/core';

    let isMac = false;
    try {
        isMac = platform() === 'macos';
    } catch {
        isMac = false;
    }

    const fullLinks = [
        {
            href: "/library",
            label: "Library",
        },
        {
            href: "/import",
            label: "Import",
        },
        ...(isMac ? [{ href: "/lyrics", label: "Lyrics" }] : []),
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
        const apiKeyOk = configStore.current?.translationProvider === 'openai'
            ? !!configStore.current?.openaiApiKey
            : !!configStore.current?.geminiApiKey;

        if (!apiKeyOk || !configStore.current?.targetLanguageId) {
            return configOnlyLinks;
        } else {
            return fullLinks;
        }
    })

    // Only redirect if on root path, otherwise respect the current URL
    let initialRedirectDone = false;
    $effect(() => {
        if (initialRedirectDone) return;
        if (configStore.current === undefined) return; // Wait for config to load

        initialRedirectDone = true;
        const currentPath = window.location.pathname;

        // Only redirect from root or if config is incomplete
        const apiKeyOk = configStore.current?.translationProvider === 'openai'
            ? !!configStore.current?.openaiApiKey
            : !!configStore.current?.geminiApiKey;
        const configComplete = apiKeyOk && configStore.current?.targetLanguageId;

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

    // When the app returns to the foreground, nudge sync: on iOS the system
    // tears down the embedded engine's sockets while suspended, so the backend
    // restarts it if it became unreachable. No-op when sync is off/healthy.
    onMount(() => {
        let waking = false;
        const onVisible = async () => {
            if (document.visibilityState !== "visible" || waking) return;
            waking = true;
            try {
                await invoke("sync_wake");
            } catch (e) {
                console.warn("sync_wake failed", e);
            } finally {
                waking = false;
            }
        };
        document.addEventListener("visibilitychange", onVisible);
        return () => document.removeEventListener("visibilitychange", onVisible);
    });
</script>

<svelte:window onresize={handleResize} />

<div bind:this={nav}>
    <Nav {links}>
        {#snippet rightActions()}
            <SyncStatusButton />
            {#if links === fullLinks}
                <AnkiSyncButton />
            {/if}
        {/snippet}
    </Nav>
</div>
<!-- Subtract the bottom safe-area inset so content clears the mobile system nav
     bar; env() is 0 on desktop. mainHeight already nets out the top inset via
     nav.clientHeight (see Nav.svelte). -->
<div class="main" style="height: calc({mainHeight.value}px - env(safe-area-inset-bottom));">
    <Router />
</div>

<style>
    .main {
        height: 100%;
    }
</style>
