import { cacheDb } from './data/cache';

export const debug = {
    /**
     * Downloads the current state of the Cache DB table as a JSON file
     */
    async downloadCache(): Promise<void> {
        try {
            // Get all cache entries
            const cacheEntries = await cacheDb.queryCache.toArray();
            
            // Convert to JSON with pretty formatting
            const jsonData = JSON.stringify(cacheEntries, null, 2);
            
            // Create blob and download link
            const blob = new Blob([jsonData], { type: 'application/json' });
            const url = URL.createObjectURL(blob);
            
            // Create temporary download link
            const link = document.createElement('a');
            link.href = url;
            link.download = `flts-cache-${new Date().toISOString().replace(/[:.]/g, '-')}.json`;
            
            // Trigger download
            document.body.appendChild(link);
            link.click();
            document.body.removeChild(link);
            
            // Clean up the URL
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
            // Create file input element
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

                    // Read file content
                    const fileText = await file.text();
                    
                    // Parse JSON
                    let cacheData;
                    try {
                        cacheData = JSON.parse(fileText);
                    } catch (parseError) {
                        reject(new Error('Invalid JSON file'));
                        return;
                    }

                    // Validate data format
                    if (!Array.isArray(cacheData)) {
                        reject(new Error('Cache data must be an array'));
                        return;
                    }

                    // Validate and normalize each cache entry
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
                        // If createdAt is missing or invalid, use current time
                        if (!entry.createdAt || typeof entry.createdAt !== 'number') {
                            entry.createdAt = currentTime;
                        }
                    }

                    // Clear existing cache and import new data
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

            // Trigger file selection dialog
            fileInput.click();
        });
    }
};
