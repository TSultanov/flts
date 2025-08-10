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
    private originalWordCache = new Map<string, UUID>();
    private translatedWordCache = new Map<string, UUID>();

    private async prepopulateCaches(originalLanguageCode: string, targetLanguageCode: string) {
        if (this.languageCache.has(originalLanguageCode.toLowerCase()) && this.languageCache.has(targetLanguageCode.toLowerCase())) {
            return;
        }

        const startTime = performance.now();
        console.log(`Dictionary: prepopulateCaches starting for ${originalLanguageCode} -> ${targetLanguageCode}`);

        await dictionaryDb.transaction(
            'r',
            [
                dictionaryDb.languages,
                dictionaryDb.words,
                dictionaryDb.wordTranslations,
            ],
            async () => {
                let stepStartTime = performance.now();
                await dictionaryDb.languages.each(l => {
                    this.languageCache.set(l.code.toLowerCase(), l.uid);
                });
                console.log(`Dictionary: languages cache population took ${(performance.now() - stepStartTime).toFixed(2)}ms`);

                const originalLanguageUid = this.languageCache.get(originalLanguageCode.toLowerCase());
                if (!originalLanguageUid) {
                    console.log(`Dictionary: original language ${originalLanguageCode} not found in cache`);
                    return;
                }

                const targetLanguageUid = this.languageCache.get(targetLanguageCode.toLowerCase());
                if (!targetLanguageUid) {
                    console.log(`Dictionary: target language ${targetLanguageCode} not found in cache`);
                    return;
                }

                stepStartTime = performance.now();
                let wordsCount = 0;
                await dictionaryDb.words.where("originalLanguageUid").equals(originalLanguageUid).each(ow => {
                    this.originalWordCache.set(`${originalLanguageUid}_${ow.originalNormalized}`, ow.uid);
                    wordsCount++;
                });
                console.log(`Dictionary: original words cache population took ${(performance.now() - stepStartTime).toFixed(2)}ms (${wordsCount} words)`);

                stepStartTime = performance.now();
                let translationsCount = 0;
                await dictionaryDb.wordTranslations.where("translationLanguageUid").equals(targetLanguageUid)
                    .each(tw => {
                        this.translatedWordCache.set(`${targetLanguageUid}_${tw.originalWordUid}_${tw.translationNormalized}`, tw.uid);
                        translationsCount++;
                    });
                console.log(`Dictionary: word translations cache population took ${(performance.now() - stepStartTime).toFixed(2)}ms (${translationsCount} translations)`);
            }
        );

        const totalTime = performance.now() - startTime;
        console.log(`Dictionary: prepopulateCaches total time: ${totalTime.toFixed(2)}ms for ${originalLanguageCode} -> ${targetLanguageCode}`);
    }

    private getCachedTranslation(
        originalWord: string,
        originalLanguageCode: string,
        targetWord: string,
        targerLanguageCode: string): UUID | undefined {
        const originalLanguageUid = this.languageCache.get(originalLanguageCode.toLowerCase());
        if (!originalLanguageUid) {
            return;
        }

        const targetLanguageUid = this.languageCache.get(targerLanguageCode.toLowerCase());
        if (!targetLanguageUid) {
            return;
        }

        const originalWordUid = this.originalWordCache.get(`${originalLanguageUid}_${originalWord.toLowerCase()}`);
        if (!originalWordUid) {
            return;
        }

        const targetWordUid = this.translatedWordCache.get(`${targetLanguageUid}_${originalWordUid}_${targetWord.toLowerCase()}`);
        return targetWordUid;
    }

    async addTranslation(
        originalWord: string,
        originalLanguageCode: string,
        targetWord: string,
        targerLanguageCode: string,
    ) {
        let start = performance.now();
        await this.prepopulateCaches(originalLanguageCode, targerLanguageCode);
        const cachedResult = this.getCachedTranslation(originalWord, originalLanguageCode, targetWord, targerLanguageCode);
        if (cachedResult) {
            return cachedResult;
        }

        start = performance.now();
        const result = await dictionaryDb.transaction(
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
        );
        return result;
    }

    private getLanguageUid(code: string) {
        return this.getOrCreateCached(
            this.languageCache,
            code.toLowerCase(),
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
            `${originalLanguageUid}_${originalWord.toLowerCase()}`,
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
            `${targetLanguageUid}_${originalWordUid}_${targetWord.toLowerCase()}`,
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
        cache: Map<string, UUID>,
        key: TKey,
        cacheKey: string,
        dbGetter: (key: TKey) => Promise<UUID>
    ) {
        const cached = cache.get(cacheKey);
        if (cached) {
            return cached;
        }

        const uid = await dbGetter(key);
        cache.set(cacheKey, uid);
        return uid;
    }
}

export const dictionary = new Dictionary();