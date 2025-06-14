import { db } from './db';

export async function setCache<T>(key: string, data: T) {
    await db.queryCache.put({
        hash: key,
        value: data
    });
}

export async function getCached<T>(key: string) : Promise<T | null> {
    const data = await db.queryCache.get(key);
    return data?.value;
}