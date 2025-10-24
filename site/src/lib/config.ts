import { invoke } from '@tauri-apps/api/core';
import { eventToReadable } from './data/tauri';

export type Model = {
    id: number,
    name: string,
}

export type Language = {
    id: string,
    name: string,
    localName?: string,
}

export type Config = {
    targetLanguageId?: string,
    geminiApiKey?: string,
    model: number,
    libraryPath?: string,
}

export async function getModels(): Promise<Model[]> {
    let models = await invoke<Model[]>("get_models");
    return models;
}

export async function getLanguages() {
    let languages = await invoke<Language[]>("get_languages");
    return languages;
}

export async function setConfig(config: Config) {
    await invoke("update_config", { config: config });
}

export const configStore = eventToReadable<Config>("config_updated", "get_config");
