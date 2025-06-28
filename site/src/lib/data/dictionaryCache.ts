import { db, generateUID, type UUID } from "./db";

const languageCache = new Map<string, UUID>();
const originalWordCache = new Map<{
    languageUid: UUID,
    word: string
}, UUID>();
const translatedWordCache = new Map<{
    languageUid: UUID,
    originalWordUid: UUID,
    word: string
}, UUID>();

export const dictionaryCache = {
    getLanguage: async (name: string) => {
        const cached = languageCache.get(name.toLowerCase());
        if (cached) {
            return cached;
        }

        const newUid = await db.transaction(
            'rw',
            [
                db.languages,
            ],
            async () => {
                const existingLanguage = await db.languages.where("name").equals(name.toLowerCase()).first();

                if (existingLanguage) {
                    return existingLanguage.uid;
                }

                const uid = generateUID();
                await db.languages.add({
                    name: name.toLowerCase(),
                    uid,
                    createdAt: Date.now(),
                });

                return uid;
            }
        );
        languageCache.set(name.toLowerCase(), newUid);
        return newUid;
    },

    getOriginalWordUid: async (languageUid: UUID, word: string) => {
        const cached = originalWordCache.get({ languageUid, word: word.toLowerCase() });
        if (cached) {
            return cached;
        }

        const newUid = await db.transaction(
            'rw',
            [
                db.words
            ],
            async () => {
                const dictWord = await db.words
                    .where("originalNormalized").equals(word.toLowerCase())
                    .and(w => w.originalLanguageUid == languageUid).first();

                if (dictWord) {
                    return dictWord.uid;
                }

                const uid = generateUID();
                await db.words.add({
                    originalLanguageUid: languageUid,
                    original: word,
                    originalNormalized: word.toLowerCase(),
                    uid,
                    createdAt: Date.now(),
                });

                return uid;
            }
        )
        originalWordCache.set({ languageUid, word: word.toLowerCase() }, newUid);
        return newUid;
    },

    getTranslatedWordUid: async (languageUid: UUID, originalWordUid: UUID, word: string) => {
        const cached = translatedWordCache.get({ languageUid, originalWordUid, word: word.toLowerCase() });
        if (cached) {
            return cached;
        }

        const newUid = await db.transaction(
            'rw',
            [
                db.wordTranslations
            ],
            async () => {
                const existingTranslation = await db.wordTranslations
                    .where("originalWordUid").equals(originalWordUid)
                    .and(wt => wt.translationNormalized === word.toLowerCase())
                    .and(wt => wt.languageUid == languageUid).first();

                if (existingTranslation) {
                    return existingTranslation.uid;
                }

                const uid = generateUID();
                await db.wordTranslations.add({
                    languageUid,
                    originalWordUid,
                    translation: word,
                    translationNormalized: word.toLowerCase(),
                    uid,
                    createdAt: Date.now(),
                });

                return uid;
            });
        translatedWordCache.set({ languageUid, originalWordUid, word: word.toLowerCase() }, newUid);
        return newUid;
    }
};