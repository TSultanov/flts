import Dexie, { type EntityTable } from "dexie";

export interface Cache {
    hash: string,
    createdAt: number,
    value: any,
}

export type CacheDB = Dexie & {
    queryCache: EntityTable<Cache, 'hash'>,
};

export const cacheDb = new Dexie('cache', {
    chromeTransactionDurability: "relaxed",
    cache: "immutable",
}) as CacheDB;

cacheDb.version(1).stores({
    queryCache: '&hash',
});

export async function setCache<T>(key: string, data: T) {
    await cacheDb.queryCache.put({
        hash: key,
        createdAt: Date.now(),
        value: data,
    });
}

export async function getCached<T>(key: string) : Promise<T | null> {
    const data = await cacheDb.queryCache.get(key);
    return data?.value;
}
