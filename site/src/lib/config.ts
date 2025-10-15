import { invoke } from '@tauri-apps/api/core';

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
    geminiApiKey: string,
    targetLanguage: string,
    model: number,
}

export async function getModels(): Promise<Model[]> {
    let models = await invoke<Model[]>("get_models");
    return models;
}

export async function getLanguages() {
    let languages = await invoke<Language[]>("get_languages");
    return languages;
}

export async function setConfig(config:Config) {
    // await store.setItem('config', config);
}

export async function getConfig() {

}