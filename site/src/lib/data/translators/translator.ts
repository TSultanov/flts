import { getConfig } from "../../config"
import { db, type DB, generateUID } from "../db"
import { GoogleTranslator } from "./google"

export type Grammar = {
    originalInitialForm: string,
    targetInitialForm: string,
    partOfSpeech: string
    plurality: string,
    person: string,
    tense: string,
    case: string,
    other: string
}

export type WordTranslation = {
    original: string,
    isPunctuation: boolean,
    isStandalonePunctuation: boolean,
    isOpeningParenthesis: boolean,
    isClosingParenthesis: boolean,
    translations: string[],
    note: string,
    grammar: Grammar,
};

export type SentenceTranslation = {
    words: WordTranslation[],
    fullTranslation: string,
}

export type ParagraphTranslation = {
    sentences: SentenceTranslation[],
    sourceLanguage: string,
    targetLanguage: string,
}

export type DictionaryRequest = {
    paragraph: string,
}

export interface Translator {
    getCachedTranslation(p: DictionaryRequest): Promise<ParagraphTranslation | null>,
    getTranslation(p: DictionaryRequest): Promise<ParagraphTranslation>
}

export type ModelId =
"gemini-2.5-flash-preview-05-20" |
"gemini-2.5-pro-preview-06-05"

export type ModelProvider = "Google"

type ModelMeta = {
    id: ModelId,
    name: string,
    provider: ModelProvider,
}

export const models: ModelMeta[] = [
    {
        id: "gemini-2.5-flash-preview-05-20",
        name: "gemini-2.5-flash-preview-05-20",
        provider: "Google",
    },
    {
        id: "gemini-2.5-pro-preview-06-05",
        name: "gemini-2.5-pro-preview-06-05",
        provider: "Google",
    },
]

export async function getTranslator(db: DB, targetLanguage: string, model: ModelId): Promise<Translator> {
    const m = models.find(m => m.id === model);
    if (!m) {
        throw new Error(`Cannot find model '${model}'`)
    }
    const config = await getConfig();
    switch (m.provider) {
        case "Google": {
            return new GoogleTranslator(config.geminiApiKey, config.targetLanguage, db, model);
        }
    }

    throw new Error(`Unknown provider ${m.provider}`);
}

export async function addTranslation(paragraphId: number, model: ModelId) {
    await db.directTranslationRequests.add({
        paragraphId,
        model,
    });
}