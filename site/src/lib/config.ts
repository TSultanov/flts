import localforage from "localforage"

export type Config = {
    apiKey: string,
    targetLanguage: string,
    model: string,
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