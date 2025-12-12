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
    } from "./config";
    import { open } from "@tauri-apps/plugin-dialog";

    let translationProvider: TranslationProvider = $derived(
        $configStore?.translationProvider ?? 'google',
    );
    let geminiApiKey: string | undefined = $derived($configStore?.geminiApiKey);
    let openaiApiKey: string | undefined = $derived($configStore?.openaiApiKey);
    let targetLanguage: string | undefined = $derived(
        $configStore?.targetLanguageId,
    );
    let libraryPath: string | undefined = $derived($configStore?.libraryPath);
    let model: number = $derived($configStore?.model ?? 0);
    let models: Model[] = $state([]);
    let providers: ProviderMeta[] = $state([]);

    let filteredModels: Model[] = $derived.by(() => {
        return models.filter((m) => {
            if (m.id === 0) return true;
            return m.provider === translationProvider;
        });
    });

    let languages = getLanguages();

    onMount(async () => {
        const [loadedModels, loadedProviders] = await Promise.all([
            getModels(),
            getTranslationProviders(),
        ]);
        models = loadedModels;
        providers = loadedProviders;
    });

    async function save() {
        await setConfig({
            translationProvider,
            geminiApiKey,
            openaiApiKey,
            targetLanguageId: targetLanguage,
            model,
            libraryPath: libraryPath ?? undefined,
        });
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
</style>
