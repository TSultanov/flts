import { invoke } from '@tauri-apps/api/core';
import { Resource } from '../data/tauri.svelte';

export type TranslationProvider = 'google' | 'openai' | 'deepseek' | 'zai';

export type Model = {
    id: string,
    name: string,
    provider?: TranslationProvider,
}

export type ProviderMeta = {
    id: TranslationProvider,
    name: string,
    defaultModel: string,
    apiKeyField: 'geminiApiKey' | 'openaiApiKey' | 'deepseekApiKey' | 'zaiApiKey',
};

export type Language = {
    id: string,
    name: string,
    localName?: string,
}

export type Config = {
    targetLanguageId?: string,
    translationProvider: TranslationProvider,
    geminiApiKey?: string,
    openaiApiKey?: string,
    deepseekApiKey?: string,
    zaiApiKey?: string,
    model: string,
    translationConcurrency?: number,
    spotifyClientId?: string,
    spotifyPreloadCount?: number,
    spotifyShowNextTrack?: boolean,
    ankiEndpoint?: string,
    ankiApiKey?: string,
    syncEnabled?: boolean,
    syncDeviceName?: string,
    tapToRevealTranslations?: boolean,
}

export function modelsForDropdown(
    models: Model[],
    provider: TranslationProvider,
    selectedId: string,
): { list: Model[]; orphan: boolean } {
    const list = models.filter((m) => m.provider === provider);
    if (selectedId !== '' && !list.some((m) => m.id === selectedId)) {
        return {
            list: [{ id: selectedId, name: selectedId, provider }, ...list],
            orphan: true,
        };
    }
    return { list, orphan: false };
}

/** Provider change or empty selection → defaultModel. Same-provider orphans stay. */
export function resolveModelSelection(
    previousProvider: TranslationProvider | undefined,
    provider: TranslationProvider,
    selectedId: string,
    defaultModel: string,
): string {
    const providerChanged =
        previousProvider !== undefined && previousProvider !== provider;
    if (!selectedId || providerChanged) {
        return defaultModel;
    }
    return selectedId;
}

export async function getModels(): Promise<Model[]> {
    let models = await invoke<Model[]>("get_models");
    return models;
}

export async function getTranslationProviders(): Promise<ProviderMeta[]> {
    let providers = await invoke<ProviderMeta[]>("get_translation_providers");
    return providers;
}

export async function getLanguages() {
    let languages = await invoke<Language[]>("get_languages");
    return languages;
}

export async function parseLanguageId(code: string): Promise<string | null> {
    return invoke<string | null>("parse_language_id", { code });
}

export async function setConfig(config: Config) {
    await invoke("update_config", { config: config });
}

export async function getConfig() {
    return await invoke<Config>("get_config");
}

export async function purgeGeminiCaches(): Promise<number> {
    return await invoke<number>("purge_gemini_caches");
}

export const configStore = new Resource<Config>("get_config", {}, [{ name: "config_updated", filter: () => true }]);
export const models = new Resource<Model[]>("get_models", {}, [], []);