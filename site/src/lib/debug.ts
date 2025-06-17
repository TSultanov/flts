import { cacheDb } from './data/cache';

export const debug = {
    /**
     * Downloads the current state of the Cache DB table as a JSON file
     */
    async downloadCache(): Promise<void> {
        try {
            const cacheEntries = await cacheDb.queryCache.toArray();
            const jsonData = JSON.stringify(cacheEntries, null, 2);
            const blob = new Blob([jsonData], { type: 'application/json' });
            const url = URL.createObjectURL(blob);
            const link = document.createElement('a');
            link.href = url;
            link.download = `flts-cache-${new Date().toISOString().replace(/[:.]/g, '-')}.json`;
            document.body.appendChild(link);
            link.click();
            document.body.removeChild(link);
            URL.revokeObjectURL(url);
            console.log(`Downloaded cache with ${cacheEntries.length} entries`);
        } catch (error) {
            console.error('Failed to download cache:', error);
            throw error;
        }
    },

    /**
     * Imports cache data from a JSON file selected by the user
     */
    async importCache(): Promise<void> {
        return new Promise((resolve, reject) => {
            const fileInput = document.createElement('input');
            fileInput.type = 'file';
            fileInput.accept = '.json';
            fileInput.onchange = async (event) => {
                try {
                    const file = (event.target as HTMLInputElement).files?.[0];
                    if (!file) {
                        reject(new Error('No file selected'));
                        return;
                    }
                    const fileText = await file.text();
                    let cacheData;
                    try {
                        cacheData = JSON.parse(fileText);
                    } catch (parseError) {
                        reject(new Error('Invalid JSON file'));
                        return;
                    }
                    if (!Array.isArray(cacheData)) {
                        reject(new Error('Cache data must be an array'));
                        return;
                    }
                    const currentTime = Date.now();
                    for (const entry of cacheData) {
                        if (!entry.hash || typeof entry.hash !== 'string') {
                            reject(new Error('Invalid cache entry: missing or invalid hash'));
                            return;
                        }
                        if (entry.value === undefined) {
                            reject(new Error('Invalid cache entry: missing value'));
                            return;
                        }
                        if (!entry.createdAt || typeof entry.createdAt !== 'number') {
                            entry.createdAt = currentTime;
                        }
                    }
                    await cacheDb.transaction('rw', cacheDb.queryCache, async () => {
                        await cacheDb.queryCache.clear();
                        await cacheDb.queryCache.bulkAdd(cacheData);
                    });
                    console.log(`Imported ${cacheData.length} cache entries`);
                    resolve();
                } catch (error) {
                    console.error('Failed to import cache:', error);
                    reject(error);
                }
            };
            fileInput.oncancel = () => {
                reject(new Error('File selection cancelled'));
            };
            fileInput.click();
        });
    }
};
