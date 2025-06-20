import Dexie, { type EntityTable } from "dexie";
import type { UUID } from "./db";
import type { ModelId } from "./translators/translator";

export interface TranslationRequest {
    id: number,
    paragraphUid: UUID,
    model: ModelId,
}

export type QueueDB = Dexie & {
    directTranslationRequests: EntityTable<TranslationRequest, 'id'>,
};

export const queueDb = new Dexie('translationQueue', {
    chromeTransactionDurability: "relaxed",
    cache: "immutable",
}) as QueueDB;

queueDb.version(1).stores({
    directTranslationRequests: '++id, paragraphUid',
});

export type {
    TranslationRequest as QueueTranslationRequest
}
