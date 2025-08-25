import { cacheDb } from './data/cache';
import { DB_FILE_NAME } from './data/sql/utils';

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
    },

    /**
     * Exports the entire library by downloading the raw SQLite (OPFS) database file.
     * Note: This is a minimal, reliable fallback that saves the DB as a single .sqlite3 file.
     */
    async exportLibrary(): Promise<void> {
        // Minimal initial implementation: export the raw SQLite DB file from OPFS.
        // This satisfies "saving the database into a file" without adding new worker APIs.
        // If OPFS is unavailable or the file doesn't exist, this will throw.
        const dbFileName = DB_FILE_NAME;
        try {
            const storage: any = (navigator as any).storage;
            if (!storage || typeof storage.getDirectory !== 'function') {
                throw new Error('OPFS is not supported in this browser (navigator.storage.getDirectory is unavailable)');
            }
            // Access the origin-private file system and read the sqlite DB file bytes
            // Path used by the worker: new OpfsDb('/db3.sqlite3') maps to OPFS root entry 'db3.sqlite3'
            // so we read that file directly here.
            const root = await storage.getDirectory();
            const handle: FileSystemFileHandle = await (root as FileSystemDirectoryHandle).getFileHandle(dbFileName, { create: false });
            const file = await handle.getFile();
            const arrayBuffer = await file.arrayBuffer();

            // Trigger download
            const blob = new Blob([arrayBuffer], { type: 'application/vnd.sqlite3' });
            const url = URL.createObjectURL(blob);
            const link = document.createElement('a');
            link.href = url;
            link.download = `flts-db-${new Date().toISOString().replace(/[:.]/g, '-')}.sqlite3`;
            document.body.appendChild(link);
            link.click();
            document.body.removeChild(link);
            URL.revokeObjectURL(url);
            console.log('Exported SQLite DB from OPFS');
        } catch (err) {
            console.error('Failed to export library DB:', err);
            throw err;
        }
    },

    /**
     * Imports a library by replacing the raw SQLite (OPFS) database file with a user-provided .sqlite3 file.
     * After a successful import, the page reloads to reinitialize the worker with the new database.
     */
    async importLibrary(): Promise<void> {
        // Minimal initial implementation: replace the OPFS SQLite DB file with a user-provided .sqlite3 file
        // and reload the page so the worker picks up the new DB content.
        const dbFileName = DB_FILE_NAME;
        return new Promise((resolve, reject) => {
            const input = document.createElement('input');
            input.type = 'file';
            input.accept = '.sqlite3,.db,application/vnd.sqlite3,application/octet-stream';
            input.onchange = async (ev) => {
                try {
                    const storage: any = (navigator as any).storage;
                    if (!storage || typeof storage.getDirectory !== 'function') {
                        throw new Error('OPFS is not supported in this browser (navigator.storage.getDirectory is unavailable)');
                    }
                    const file = (ev.target as HTMLInputElement).files?.[0];
                    if (!file) {
                        reject(new Error('No file selected'));
                        return;
                    }
                    const buf = await file.arrayBuffer();

                    // Write to OPFS temp file; worker will swap it on next load
                    const root = await storage.getDirectory();
                    const tempName = `${dbFileName}.import`;
                    const handle: FileSystemFileHandle = await (root as FileSystemDirectoryHandle).getFileHandle(tempName, { create: true });
                    const writable = await handle.createWritable();
                    try {
                        await writable.truncate(0);
                        await writable.write(buf);
                    } finally {
                        await writable.close();
                    }

                    console.log('Staged imported SQLite DB into OPFS temp file');
                    // Reload the app to ensure the worker reconnects to the updated DB
                    resolve();
                    // Perform reload at the end of the call stack to allow caller to finish any UI updates
                    setTimeout(() => location.reload(), 50);
                } catch (e) {
                    console.error('Failed to import library DB:', e);
                    reject(e);
                }
            };
            input.oncancel = () => reject(new Error('File selection cancelled'));
            input.click();
        });
    },

    /**
     * Removes the SQLite (OPFS) database file and any staged import file, then reloads the app.
     */
    async removeDatabase(): Promise<void> {
        const dbFileName = DB_FILE_NAME;
        try {
            const storage: any = (navigator as any).storage;
            if (!storage || typeof storage.getDirectory !== 'function') {
                throw new Error('OPFS is not supported in this browser (navigator.storage.getDirectory is unavailable)');
            }
            const root = await storage.getDirectory();

            // Helper to remove or truncate a file if removal fails (e.g., due to locks)
            const removeOrTruncate = async (name: string) => {
                try {
                    // Try direct remove; TS types for removeEntry may be missing, cast to any
                    await (root as any).removeEntry(name);
                    return;
                } catch (e) {
                    // Fallback: truncate the file to zero bytes
                    try {
                        const handle: FileSystemFileHandle = await (root as FileSystemDirectoryHandle).getFileHandle(name, { create: false });
                        const writable = await handle.createWritable();
                        try {
                            await writable.truncate(0);
                        } finally {
                            await writable.close();
                        }
                    } catch {
                        // If the file doesn't exist or cannot be opened, ignore
                    }
                }
            };

            // Remove main DB and any staged import file
            await removeOrTruncate(dbFileName);
            await removeOrTruncate(`${dbFileName}.import`);

            console.log('Database removed from OPFS');
            // Reload to reinitialize worker with a fresh DB
            setTimeout(() => location.reload(), 50);
        } catch (err) {
            console.error('Failed to remove database:', err);
            throw err;
        }
    },

};
