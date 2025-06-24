import { getConfig } from "../../config"
import { type UUID } from "../db"
import { queueDb } from "../queueDb"
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
    isPunctuation?: boolean,
    isStandalonePunctuation?: boolean,
    isOpeningParenthesis?: boolean,
    isClosingParenthesis?: boolean,
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
"gemini-2.5-flash" |
"gemini-2.5-pro" |
"gemini-2.5-flash-lite-preview-06-17"

export type ModelProvider = "Google"

type ModelMeta = {
    id: ModelId,
    name: string,
    provider: ModelProvider,
}

export const models: ModelMeta[] = [
    {
        id: "gemini-2.5-flash",
        name: "gemini-2.5-flash",
        provider: "Google",
    },
    {
        id: "gemini-2.5-pro",
        name: "gemini-2.5-pro",
        provider: "Google",
    },
    {
        id: "gemini-2.5-flash-lite-preview-06-17",
        name: "gemini-2.5-flash-lite-preview-06-17",
        provider: "Google",
    },
]

export async function getTranslator(targetLanguage: string, model: ModelId): Promise<Translator> {
    const m = models.find(m => m.id === model);
    if (!m) {
        throw new Error(`Cannot find model '${model}'`)
    }
    const config = await getConfig();
    switch (m.provider) {
        case "Google": {
            return new GoogleTranslator(config.geminiApiKey, config.targetLanguage, model);
        }
    }

    throw new Error(`Unknown provider ${m.provider}`);
}

export async function addTranslation(paragraphUid: UUID, model: ModelId) {
    // const paragraph = await db.paragraphs.where('uid').equals(paragraphUid).first();
    // if (!paragraph) {
    //     console.warn(`Cannot add translation: paragraph with uid ${paragraphUid} not found`);
    //     return;
    // }
    
    await queueDb.directTranslationRequests.add({
        paragraphUid,
        model,
    });
}