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

    const library = new Library();
    setContext("library", library);

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

<Nav {links}>
    {#snippet rightActions()}
        <SyncStatusButton />
        {#if links === fullLinks}
            <AnkiSyncButton />
        {/if}
    {/snippet}
</Nav>
<div class="main">
    <Router />
</div>

<style>
    /* #app is a full-height flex column; .main takes whatever the nav
       leaves, tracked by the browser itself. Measuring window.innerHeight
       from JS (the previous approach) raced WKWebView's launch sizing and
       the async application of safe-area insets — neither of which fires a
       resize event — leaving a random stale height for the whole session.
       The bottom margin keeps content clear of the mobile home indicator;
       env() is 0 on desktop. The top inset is absorbed by the nav's own
       safe-area padding (see Nav.svelte). */
    .main {
        flex: 1 1 auto;
        min-height: 0;
        margin-bottom: env(safe-area-inset-bottom);
    }
</style>
