import Dexie, { type EntityTable } from "dexie";
import type { ModelId } from "./translators/translator";
import { getConfig } from "../config";
import type { BookChapterParagraphId, BookId, DatabaseSchema } from "./evolu/schema";
import type { Books } from "./evolu/book";
import type { Evolu } from "@evolu/common";

export interface TranslationRequest {
    id: number,
    bookId: BookId,
    paragraphId: BookChapterParagraphId,
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
    directTranslationRequests: '++id, bookId, paragraphId',
});

export type {
    TranslationRequest as QueueTranslationRequest
}

export class TranslationQueue
{
    constructor(private evolu: Evolu<DatabaseSchema>, private books: Books) {}

    async cleanupTranslationRequests(bookId: BookId) {
        await queueDb.directTranslationRequests.where('bookId').equals(bookId).delete();
    }

    async scheduleTranslation(bookId: BookId, paragraphId: BookChapterParagraphId) {
        const config = await getConfig();
        await queueDb.directTranslationRequests.add({
            bookId,
            paragraphId,
            model: config.model,
        });
    }

    async scheduleFullBookTranslation(bookId: BookId) {
        const config = await getConfig();
        const untranslatedParagraphs = await this.evolu.loadQuery(this.books.nonTranslatedParagraphsIds(bookId));

        for (const p of untranslatedParagraphs) {
            await queueDb.directTranslationRequests.add({
                bookId,
                paragraphId: p.paragraphId,
                model: config.model,
            });
        }
    }

    async hasRequest(paragraphId: BookChapterParagraphId) {
        return await queueDb.directTranslationRequests
            .where("paragraphId").equals(paragraphId)
            .count() > 0;
    }

    top(limit: number) {
        return queueDb.directTranslationRequests.limit(limit).toArray();
    }

    async removeRequest(id: number){
        await queueDb.directTranslationRequests.delete(id);
    }
}