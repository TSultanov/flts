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

    let initialRedirectDone = false;
    $effect(() => {
        if (initialRedirectDone) return;
        if (configStore.current === undefined) return;

        initialRedirectDone = true;
        const currentPath = window.location.pathname;

        const apiKeyOk = configStore.current?.translationProvider === 'openai'
            ? !!configStore.current?.openaiApiKey
            : !!configStore.current?.geminiApiKey;
        const configComplete = apiKeyOk && configStore.current?.targetLanguageId;

        if (!configComplete) {
            if (currentPath !== '/config') {
                navigate("/config");
            }
        } else if (currentPath === '/' || currentPath === '') {
            navigate("/library");
        }
    });

    const library = new Library();
    setContext("library", library);

    // iOS tears down the embedded engine's sockets while suspended, so the
    // backend restarts it on wake if unreachable.
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
    /* Height must stay browser-tracked: measuring window.innerHeight from
       JS races WKWebView's launch sizing and the async safe-area insets,
       neither of which fires a resize event, stranding a stale height for
       the session. The bottom margin clears the mobile home indicator; the
       top inset is absorbed by the nav (see Nav.svelte). */
    .main {
        flex: 1 1 auto;
        min-height: 0;
        margin-bottom: env(safe-area-inset-bottom);
    }
</style>
