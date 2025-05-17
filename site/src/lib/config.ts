import localforage from "localforage"

export type Config = {
    apiKey: string,
    targetLanguage: string
}

let store = localforage.createInstance({storeName: "config"});

export async function setConfig(config:Config) {
    await store.setItem('config', config);
}

export async function getConfig() {
    return await store.getItem('config') as Config
}