import Dexie, { type EntityTable } from "dexie";
import type { ModelId } from "./translators/translator";
import type { UUID } from "./v2/db";
import { getConfig } from "../config";
import { sqlBooks } from "./sql/book";
import { readableToPromise } from "./sql/utils";

export interface TranslationRequest {
    id: number,
    bookUid: UUID,
    paragraphUid: UUID,
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
    directTranslationRequests: '++id, bookUid, paragraphUid',
});

export type {
    TranslationRequest as QueueTranslationRequest
}

export const translationQueue = {
    cleanupTranslationRequests: async (bookUid: UUID) => {
        await queueDb.directTranslationRequests.where('bookUid').equals(bookUid).delete();
    },

    scheduleTranslation: async (bookUid: UUID, paragraphUid: UUID) => {
        const config = await getConfig();
        await queueDb.directTranslationRequests.add({
            bookUid,
            paragraphUid,
            model: config.model,
        });
    },

    scheduleFullBookTranslation: async (bookUid: UUID) => {
        const config = await getConfig();
        const untranslatedParagraphs = await readableToPromise(sqlBooks.getNotTranslatedParagraphsUids(bookUid));

        if (untranslatedParagraphs) {
            for (const p of untranslatedParagraphs) {
                await queueDb.directTranslationRequests.add({
                    bookUid,
                    paragraphUid: p,
                    model: config.model,
                });
            }
        }
    },

    hasRequest: async (bookUid: UUID, paragraphUid: UUID) => {
        return await queueDb.directTranslationRequests
            .where("bookUid").equals("bookUid")
            .and(r => r.paragraphUid === paragraphUid)
            .count() > 0;
    },

    top: (limit: number) => {
        return queueDb.directTranslationRequests.limit(limit).toArray();
    },

    removeRequest: async (id: number) => {
        await queueDb.directTranslationRequests.delete(id);
    }
}