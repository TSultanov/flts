<script lang="ts">
    import { onMount } from "svelte";
    import {
        configStore,
        getLanguages,
        getModels,
        getTranslationProviders,
        setConfig,
        type Model,
        type ProviderMeta,
        type TranslationProvider,
    } from "./store";
    import { open } from "@tauri-apps/plugin-dialog";
    import { platform } from "@tauri-apps/plugin-os";
    import { invoke } from "@tauri-apps/api/core";
    import {
        spotifyWebConnect,
        spotifyWebDisconnect,
        spotifyWebStatus,
        type SpotifyWebStatus,
    } from "../spotify/queueStore";

    const SPOTIFY_DASHBOARD_URL = 'https://developer.spotify.com/dashboard';
    const SPOTIFY_REDIRECT_URI = 'http://127.0.0.1:53682/callback';

    let translationProvider: TranslationProvider = $derived(
        configStore.current?.translationProvider ?? 'google',
    );
    let geminiApiKey: string | undefined = $derived(configStore.current?.geminiApiKey);
    let openaiApiKey: string | undefined = $derived(configStore.current?.openaiApiKey);
    let targetLanguage: string | undefined = $derived(
        configStore.current?.targetLanguageId,
    );
    let libraryPath: string | undefined = $derived(configStore.current?.libraryPath);
    let model: number = $derived(configStore.current?.model ?? 0);
    let models: Model[] = $state([]);
    let providers: ProviderMeta[] = $state([]);

    let spotifyClientId: string = $derived(
        configStore.current?.spotifyClientId ?? '',
    );
    let spotifyPreloadCount: number = $derived(
        configStore.current?.spotifyPreloadCount ?? 1,
    );
    let spotifyShowNextTrack: boolean = $derived(
        configStore.current?.spotifyShowNextTrack ?? true,
    );
    let spotifyStatus: SpotifyWebStatus = $state({
        connected: false,
        premiumRequired: false,
        lastError: null,
    });
    let spotifyBusy: boolean = $state(false);
    let spotifyError: string | null = $state(null);
    // Spotify integration is macOS-only — same constraint as LyricsView.
    let isMac: boolean = $state(false);
    let redirectCopied: boolean = $state(false);
    let redirectCopyTimer: ReturnType<typeof setTimeout> | undefined;

    async function openDashboard(e: MouseEvent) {
        e.preventDefault();
        try {
            await invoke('open_external_url', { url: SPOTIFY_DASHBOARD_URL });
        } catch (err) {
            spotifyError = `Could not open browser: ${err}`;
        }
    }

    async function copyRedirectUri() {
        try {
            await navigator.clipboard.writeText(SPOTIFY_REDIRECT_URI);
            redirectCopied = true;
            if (redirectCopyTimer) clearTimeout(redirectCopyTimer);
            redirectCopyTimer = setTimeout(() => {
                redirectCopied = false;
            }, 1500);
        } catch (err) {
            spotifyError = `Could not copy to clipboard: ${err}`;
        }
    }

    let filteredModels: Model[] = $derived.by(() => {
        return models.filter((m) => {
            if (m.id === 0) return true;
            return m.provider === translationProvider;
        });
    });

    let languages = getLanguages();

    onMount(async () => {
        try {
            isMac = platform() === 'macos';
        } catch {
            isMac = false;
        }
        const [loadedModels, loadedProviders] = await Promise.all([
            getModels(),
            getTranslationProviders(),
        ]);
        models = loadedModels;
        providers = loadedProviders;
        if (isMac) {
            spotifyStatus = await spotifyWebStatus();
        }
    });

    async function save() {
        await setConfig({
            translationProvider,
            geminiApiKey,
            openaiApiKey,
            targetLanguageId: targetLanguage,
            model,
            libraryPath: libraryPath ?? undefined,
            spotifyClientId: spotifyClientId.trim() || undefined,
            spotifyPreloadCount,
            spotifyShowNextTrack,
        });
    }

    async function connectSpotify() {
        spotifyError = null;
        if (!spotifyClientId.trim()) {
            spotifyError = 'Set your Spotify client_id first, then click Save.';
            return;
        }
        spotifyBusy = true;
        try {
            // Persist the client_id before the auth flow so a successful resume
            // on next launch picks it up. The backend won't poll without it.
            await save();
            await spotifyWebConnect(spotifyClientId.trim());
            spotifyStatus = await spotifyWebStatus();
        } catch (e) {
            spotifyError = String(e);
        } finally {
            spotifyBusy = false;
        }
    }

    async function disconnectSpotify() {
        spotifyBusy = true;
        try {
            await spotifyWebDisconnect();
            spotifyStatus = await spotifyWebStatus();
        } finally {
            spotifyBusy = false;
        }
    }

    $effect(() => {
        // Keep model selection consistent with provider.
        const providerMeta = providers.find((p) => p.id === translationProvider);
        if (!providerMeta) return;

        const selectedModel = models.find((m) => m.id === model);
        const providerMismatch = selectedModel && selectedModel.id !== 0 && selectedModel.provider !== translationProvider;
        if (model === 0 || !selectedModel || providerMismatch) {
            model = providerMeta.defaultModelId;
        }
    });

    async function selectDirectory() {
        libraryPath =
            (await open({
                multiple: false,
                directory: true,
            })) ?? libraryPath;
    }
</script>

{#await languages}
    Loading...
{:then languages}
    <div class="container">
        <div class="config-form">
            <label for="targetlanguage">Target Language</label>
            <select id="targetlanguage" bind:value={targetLanguage}>
                {#each languages as language}
                    <option value={language.id}
                        >{language.name}
                        {language.localName
                            ? `(${language.localName})`
                            : ""}</option
                    >
                {/each}
            </select>

            <label for="provider">Provider</label>
            <select id="provider" bind:value={translationProvider}>
                {#if providers.length === 0}
                    <option value="google">Google</option>
                    <option value="openai">OpenAI</option>
                {:else}
                    {#each providers as provider}
                        <option value={provider.id}>{provider.name}</option>
                    {/each}
                {/if}
            </select>

            {#if translationProvider === 'google'}
                <label for="apikey">Gemini API KEY</label>
                <input id="apikey" type="text" bind:value={geminiApiKey} />
            {:else if translationProvider === 'openai'}
                <label for="openai">OpenAI API KEY</label>
                <input id="openai" type="text" bind:value={openaiApiKey} />
            {/if}

            <label for="model">Model</label>
            <select id="model" bind:value={model}>
                {#each filteredModels as model}
                    <option value={model.id}>{model.name}</option>
                {/each}
            </select>

            <label for="library">Library</label>
            <input id="library" type="text" bind:value={libraryPath} />
            <button id="selectDirectory" onclick={selectDirectory}
                >Select directory</button
            >

            {#if isMac}
                <details class="spotify-section">
                    <summary>Spotify (optional)</summary>
                    <div class="spotify-grid">
                        <p class="hint">
                            Enables "Up next" and silent preload of lyrics +
                            translation for upcoming tracks while playing a
                            playlist or album. Create a Spotify Developer app at
                            <a
                                href={SPOTIFY_DASHBOARD_URL}
                                onclick={openDashboard}
                                class="external"
                                >{SPOTIFY_DASHBOARD_URL}</a
                            >, add
                            <span class="copyable">
                                <code>{SPOTIFY_REDIRECT_URI}</code><button
                                    type="button"
                                    class="copy-btn"
                                    onclick={copyRedirectUri}
                                    title="Copy to clipboard"
                                    aria-label="Copy redirect URI">{redirectCopied
                                        ? '✓'
                                        : '⧉'}</button
                                >
                            </span>
                            as a redirect URI, and paste the client ID below.
                            Premium is required for queue access.
                        </p>

                        <label for="spotifyClientId">Spotify client_id</label>
                        <input
                            id="spotifyClientId"
                            type="text"
                            bind:value={spotifyClientId}
                        />
                        {#if spotifyStatus.connected}
                            <button
                                id="spotifyAction"
                                onclick={disconnectSpotify}
                                disabled={spotifyBusy}
                            >
                                {spotifyBusy ? '...' : 'Disconnect'}
                            </button>
                        {:else}
                            <button
                                id="spotifyAction"
                                onclick={connectSpotify}
                                disabled={spotifyBusy || !spotifyClientId.trim()}
                            >
                                {spotifyBusy ? 'Connecting...' : 'Connect'}
                            </button>
                        {/if}

                        <label for="spotifyPreload">Preload tracks ahead</label>
                        <input
                            id="spotifyPreload"
                            type="number"
                            min="0"
                            max="3"
                            bind:value={spotifyPreloadCount}
                        />

                        <label for="spotifyShowNext">Show next track</label>
                        <input
                            id="spotifyShowNext"
                            type="checkbox"
                            bind:checked={spotifyShowNextTrack}
                        />

                        {#if spotifyStatus.premiumRequired}
                            <div class="spotify-notice warn">
                                Spotify Premium is required to access the queue
                                endpoint. Free accounts can connect but "Up
                                next" and preload won't work.
                            </div>
                        {/if}
                        {#if spotifyError}
                            <div class="spotify-notice err">{spotifyError}</div>
                        {:else if spotifyStatus.lastError}
                            <div class="spotify-notice err">
                                Last error: {spotifyStatus.lastError}
                            </div>
                        {/if}
                    </div>
                </details>
            {/if}

            <button id="save" onclick={save} class="primary">Save</button>
        </div>
    </div>
{/await}

<style>
    .container {
        display: flex;
        justify-content: center;
        align-items: center;
        height: 100%;
    }

    .config-form {
        max-width: 500px;
        display: grid;
        gap: 10px;
        grid-template-columns: auto 1fr auto;
        align-items: stretch;
        justify-items: stretch;
    }

    label {
        grid-column: 1/2;
    }

    input,
    select {
        grid-column: 2/4;
    }

    input#library {
        grid-column: 2/3;
    }

    button#selectDirectory {
        grid-column: 3/4;
    }

    button#save {
        grid-column: 1/4;
    }

    details.spotify-section {
        grid-column: 1/4;
        margin-top: 8px;
        border-top: 1px solid color-mix(in srgb, currentColor 15%, transparent);
        padding-top: 12px;
    }
    details.spotify-section > summary {
        cursor: pointer;
        font-weight: 600;
        padding: 2px 0;
        user-select: none;
    }
    .spotify-grid {
        display: grid;
        gap: 10px;
        grid-template-columns: auto 1fr auto;
        align-items: stretch;
        justify-items: stretch;
        margin-top: 10px;
    }
    p.hint {
        grid-column: 1/4;
        font-size: 0.85em;
        opacity: 0.8;
        margin: 0;
        line-height: 1.4;
    }
    p.hint a.external {
        color: var(--link-color, #2a6cd6);
        text-decoration: underline;
        cursor: pointer;
        font-size: inherit;
    }
    .copyable {
        display: inline-flex;
        align-items: center;
        gap: 4px;
        padding: 1px 4px 1px 6px;
        border-radius: 3px;
        background: color-mix(in srgb, currentColor 8%, transparent);
        white-space: nowrap;
        font-size: inherit;
    }
    .copyable code {
        background: none;
        padding: 0;
        font-size: inherit;
    }
    .copy-btn {
        appearance: none;
        background: none;
        border: none;
        padding: 0 4px;
        cursor: pointer;
        color: inherit;
        opacity: 0.7;
        font-size: inherit;
        line-height: 1;
    }
    .copy-btn:hover {
        opacity: 1;
    }
    button#spotifyAction {
        grid-column: 3/4;
    }
    input#spotifyClientId {
        grid-column: 2/3;
    }
    input#spotifyPreload {
        grid-column: 2/4;
        max-width: 6em;
    }
    input#spotifyShowNext {
        grid-column: 2/4;
        justify-self: start;
    }
    .spotify-notice {
        grid-column: 1/4;
        font-size: 0.85em;
        padding: 6px 10px;
        border-radius: 4px;
        background: color-mix(in srgb, currentColor 8%, transparent);
    }
    .spotify-notice.warn {
        color: #c08000;
    }
    .spotify-notice.err {
        color: var(--error-color, #b00020);
    }
</style>
