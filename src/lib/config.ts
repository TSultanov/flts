import localforage from "localforage"

type Config = {
    api_key: string
}

let store = localforage.createInstance({storeName: "config"});

export async function setConfig(config:Config) {
    await store.setItem('config', config);
}

export async function getConfig() {
    return await store.getItem('config') as Config
}