import localforage from "localforage"
import { models, type ModelId } from "./data/translators/translator";

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
    if (!config.model || models.map(m => m.id).indexOf(config.model) < 0) {
        config.model = "gemini-2.5-flash";
        await setConfig(config);
    }
    return config;
}