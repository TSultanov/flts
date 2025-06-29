import Dexie, { type EntityTable } from "dexie"
import { generateUID, type Entity, type UUID } from "./db"

type Language = Entity & {
    code: string,
}

type Word = Entity & {
    originalLanguageUid: UUID,
    original: string,
    originalNormalized: string,
}

type WordTranslation = Entity & {
    translationLanguageUid: UUID,
    originalWordUid: UUID,
    translation: string,
    translationNormalized: string,
}

type DictionaryDb = Dexie & {
    languages: EntityTable<Language, 'uid'>,
    words: EntityTable<Word, 'uid'>,
    wordTranslations: EntityTable<WordTranslation, 'uid'>,
};

const dictionaryDb = new Dexie('dictionary', {
    chromeTransactionDurability: "relaxed",
    cache: "immutable"
}) as DictionaryDb;

dictionaryDb.version(1).stores({
    languages: "&uid, code",
    words: "&uid, originalLanguageUid, original, originalNormalized",
    wordTranslations: "&uid, translationLanguageUid, originalWordUid, translation, translationNormalized",
})

class Dictionary {
    private languageCache = new Map<string, UUID>();
    private originalWordCache = new Map<{
        originalLanguageUid: UUID,
        originalWord: string
    }, UUID>();
    private translatedWordCache = new Map<{
        targetLanguageUid: UUID,
        originalWordUid: UUID,
        targetWord: string
    }, UUID>();

    async addTranslation(
        originalWord: string,
        originalLanguageCode: string,
        targetWord: string,
        targerLanguageCode: string,
    ) {
        return await dictionaryDb.transaction(
            'rw',
            [
                dictionaryDb.languages,
                dictionaryDb.words,
                dictionaryDb.wordTranslations,
            ],
            async () => {
                const originalLanguageUid = await this.getLanguageUid(originalLanguageCode);
                const targetLanguageUid = await this.getLanguageUid(targerLanguageCode);
                const originalWordUid = await this.getOriginalWordUid({
                    originalLanguageUid,
                    originalWord,
                });
                const translationUid = await this.getTranslatedWordUid({
                    targetLanguageUid,
                    originalWordUid,
                    targetWord,
                });
                return translationUid;
            }
        )
    }

    private getLanguageUid(code: string) {
        return this.getOrCreateCached(
            this.languageCache,
            code.toLowerCase(),
            async (code) => {
                const existingLanguage = await dictionaryDb.languages.where("code")
                    .equals(code).first();

                if (existingLanguage) {
                    return existingLanguage.uid;
                }

                const uid = generateUID();
                const now = Date.now();
                await dictionaryDb.languages.add({
                    code: code,
                    uid,
                    createdAt: now,
                    updatedAt: now,
                });

                return uid;
            }
        );
    }

    private getOriginalWordUid({
        originalLanguageUid,
        originalWord,
    }: {
        originalLanguageUid: UUID,
        originalWord: string,
    }) {
        return this.getOrCreateCached(
            this.originalWordCache,
            {
                originalLanguageUid,
                originalWord: originalWord.toLowerCase(),
            },
            async ({ originalLanguageUid, originalWord: wordNormalized }) => {
                const dictWord = await dictionaryDb.words
                    .where("originalNormalized").equals(wordNormalized)
                    .and(w => w.originalLanguageUid == originalLanguageUid).first();

                if (dictWord) {
                    return dictWord.uid;
                }

                const uid = generateUID();
                const now = Date.now();
                await dictionaryDb.words.add({
                    originalLanguageUid,
                    original: originalWord,
                    originalNormalized: wordNormalized,
                    uid,
                    createdAt: now,
                    updatedAt: now,
                });

                return uid;
            }
        );
    }

    private getTranslatedWordUid({
        targetLanguageUid,
        originalWordUid,
        targetWord,
    }: {
        targetLanguageUid: UUID,
        originalWordUid: UUID,
        targetWord: string
    }) {
        return this.getOrCreateCached(
            this.translatedWordCache,
            {
                targetLanguageUid,
                originalWordUid,
                targetWord: targetWord.toLowerCase(),
            },
            async ({
                targetLanguageUid,
                originalWordUid,
                targetWord: wordNormalized,
            }) => {
                const existingTranslation = await dictionaryDb.wordTranslations
                    .where("originalWordUid").equals(originalWordUid)
                    .and(wt => wt.translationNormalized === wordNormalized)
                    .and(wt => wt.translationLanguageUid == originalWordUid).first();

                if (existingTranslation) {
                    return existingTranslation.uid;
                }

                const uid = generateUID();
                const now = Date.now();
                await dictionaryDb.wordTranslations.add({
                    translationLanguageUid: targetLanguageUid,
                    originalWordUid,
                    translation: targetWord,
                    translationNormalized: wordNormalized,
                    uid,
                    createdAt: now,
                    updatedAt: now,
                });

                return uid;
            }
        )
    }

    private async getOrCreateCached<TKey, UUID>(
        cache: Map<TKey, UUID>,
        key: TKey,
        dbGetter: (key: TKey) => Promise<UUID>
    ) {
        const cached = cache.get(key);
        if (cached) {
            return cached;
        }

        const uid = await dbGetter(key);
        cache.set(key, uid);
        return uid;
    }
}

export const dictionary = new Dictionary();