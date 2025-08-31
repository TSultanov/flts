import { createIdFromString, getOrThrow, type Evolu } from "@evolu/common";
import type { DatabaseSchema, LanguageId, WordSpellingVariantId, WordTranslationSpellingVariantId } from "./schema";

export class Dictionary {
    constructor(private evolu: Evolu<DatabaseSchema>) { }

    upsertLanguage(code: string): LanguageId {
        const id = createIdFromString(code);
        const result = this.evolu.upsert("language", {
            id,
            code,
        });
        return getOrThrow(result).id;
    }

    private upsertWord(originalLanguage: LanguageId, word: string): WordSpellingVariantId {
        const wordId = createIdFromString(word.toLowerCase());
        const wordResult = this.evolu.upsert("word", {
            id: wordId,
            originalLanguageId: originalLanguage,
        });
        const upsertedId = getOrThrow(wordResult).id;
        
        const wordSpellingVariantId = createIdFromString(word);
        const wordSpellingVariantResult = this.evolu.upsert("wordSpellingVariant", {
            id: wordSpellingVariantId,
            wordId: upsertedId,
        });

        return getOrThrow(wordSpellingVariantResult).id;
    }

    private upsertTranslation(
        originalWord: WordSpellingVariantId,
        targetLanguage: LanguageId,
        targetWord: string
    ): WordTranslationSpellingVariantId {
        const wordTranslationId = createIdFromString(targetWord.toLowerCase());
        const wordTranslationResult = this.evolu.upsert("wordTranslation", {
            id: wordTranslationId,
            translationLanguageId: targetLanguage,
            originalWordVariantId: originalWord,
        });
        const upsertedId = getOrThrow(wordTranslationResult).id;

        const wordTranslationVariantId = createIdFromString(targetWord);
        const wordTranslationVariantResult = this.evolu.upsert("wordTranslationSpellingVariant", {
            id: wordTranslationVariantId,
            wordTranslationId: upsertedId
        });

        return getOrThrow(wordTranslationVariantResult).id;
    }

    addTranslation(
        originalWord: string,
        originalLanguageCode: string,
        targetWord: string,
        targetLanguageCode: string
    ): WordTranslationSpellingVariantId {
        const originalLanguageUid = this.upsertLanguage(originalLanguageCode);
        const wordId = this.upsertWord(originalLanguageUid, originalWord);
        const targetLangUid = this.upsertLanguage(targetLanguageCode);
        return this.upsertTranslation(wordId, targetLangUid, targetWord);
    }
}