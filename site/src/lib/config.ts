import localforage from "localforage"
import type { ModelId } from "./data/translators/translator";

export type Config = {
    geminiApiKey: string,
    targetLanguage: string,
    model: ModelId,
}

let store = localforage.createInstance({storeName: "config"});

export async function setConfig(config:Config) {
    await store.setItem('config', config);
}

export async function getConfig() {
    let config = await store.getItem('config') as Config
    if (!config.model) {
        config.model = "gemini-2.5-flash-preview-05-20";
    }
    return config;
}