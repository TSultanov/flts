import Dexie, { type EntityTable } from "dexie";
import type { ModelId } from "./translators/translator";
import type { UUID } from "./v2/db";
import { books, type ParagraphId } from "./v2/book.svelte";
import { getConfig } from "../config";

export interface TranslationRequest {
    id: number,
    bookUid: UUID,
    paragraphId: ParagraphId,
    model: ModelId,
}

type QueueDB = Dexie & {
    directTranslationRequests: EntityTable<TranslationRequest, 'id'>,
};

const queueDb = new Dexie('translationQueue', {
    chromeTransactionDurability: "relaxed",
    cache: "immutable",
}) as QueueDB;

queueDb.version(1).stores({
    directTranslationRequests: '++id, bookUid, paragraphId',
});

export type {
    TranslationRequest as QueueTranslationRequest
}

export const translationQueue = {
    cleanupTranslationRequests: async (bookUid: UUID) => {
        await queueDb.directTranslationRequests.where('bookUid').equals(bookUid).delete();
    },

    scheduleTranslation: async (bookUid: UUID, paragraphId: ParagraphId) => {
        const config = await getConfig();
        await queueDb.directTranslationRequests.add({
            bookUid,
            paragraphId,
            model: config.model,
        });
    },

    scheduleFullBookTranslation: async (bookUid: UUID) => {
        const config = await getConfig();
        const book = await books.getBook(bookUid);
        if (!book) {
            return;
        }
        for (const c of book.chapters) {
            for (const p of c.paragraphs) {
                await queueDb.directTranslationRequests.add({
                    bookUid,
                    paragraphId: p.id,
                    model: config.model,
                });
            }
        }
    },

    hasRequest: async (bookUid: UUID, paragraphId: ParagraphId) => {
        return await queueDb.directTranslationRequests
            .where("bookUid").equals("bookUid")
            .and(r => r.paragraphId === paragraphId) // FIXME this is not working apparently
            .count() > 0;
    },

    top: (limit: number) => {
        return queueDb.directTranslationRequests.limit(limit).toArray();
    },

    removeRequest: async (id: number) => {
        await queueDb.directTranslationRequests.delete(id);
    }
}