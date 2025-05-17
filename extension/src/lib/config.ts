import { storage } from '#imports';

type Config = {
    apiKey: string,
    to: string
}

export async function setConfig(config:Config) {
    await storage.setItem('local:config', config);
}

export async function getConfig() {
    return await storage.getItem('local:config') as Config
}