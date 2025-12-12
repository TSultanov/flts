import { invoke } from '@tauri-apps/api/core';
import { eventToReadable, getterToReadableWithEvents } from './data/tauri';

export type TranslationProvider = 'google' | 'openai';

export type Model = {
    id: number,
    name: string,
    provider?: TranslationProvider,
}

export type ProviderMeta = {
    id: TranslationProvider,
    name: string,
    defaultModelId: number,
    apiKeyField: 'geminiApiKey' | 'openaiApiKey',
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
    model: number,
    libraryPath?: string,
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

export async function setConfig(config: Config) {
    await invoke("update_config", { config: config });
}

export async function getConfig() {
    return await invoke<Config>("get_config");
}

export const configStore = eventToReadable<Config>("config_updated", "get_config");
export const models = getterToReadableWithEvents<Model[]>("get_models", {}, [], []);