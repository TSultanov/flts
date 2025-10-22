import { invoke } from '@tauri-apps/api/core';

import { listen } from '@tauri-apps/api/event';
import { readable } from 'svelte/store';

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

export async function getConfig() {
    return await invoke<Config>("get_config");
}

let config = await getConfig();

let setter: ((value: Config) => void) | null = null;
const unsub = await listen<Config>("config_updated", (event) => {
    console.log("config_updated event 1");
    if (setter) {
        console.log("config_updated event 2");
        setter(event.payload);
    }
});

export const configStore = readable<Config>(config, (set) => {
    setter = set;
    return unsub;
});