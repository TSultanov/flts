import { getConfig } from "../../config"
import { GoogleTranslator } from "./google"
import { OpenAITranslator } from "./openai"

export type Grammar = {
    originalInitialForm: string,
    targetInitialForm: string,
    partOfSpeech: string
    plurality: string | null,
    person: string | null,
    tense: string | null,
    case: string | null,
    other: string | null,
}

export type WordTranslation = {
    original: string,
    isPunctuation: boolean,
    isStandalonePunctuation: boolean | null,
    isOpeningParenthesis: boolean | null,
    isClosingParenthesis: boolean | null,
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
"gemini-2.5-flash-lite-preview-06-17" |
"gpt-4.1" |
"gpt-4o-mini" |
"gpt-4o"

export type ModelProvider = "Google" | "OpenAI"

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
        id: "gpt-4.1",
        name: "GPT 4.1",
        provider: "OpenAI",
    },
    {
        id: "gpt-4o-mini",
        name: "GPT 4o mini",
        provider: "OpenAI",
    },
    {
        id: "gpt-4o",
        name: "GPT 4o",
        provider: "OpenAI",
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
        case "OpenAI": {
            return new OpenAITranslator(config.openAIApiKey, config.targetLanguage, model);
        }
    }

    throw new Error(`Unknown provider ${m.provider}`);
}

export function getPrompt(to: string) {
    return `You are given a paragraph in a foreign language. The goal is to construct a translation which can be used by somebody who speaks the ${to} language to learn the original language.
        For each sentence provide a good, but close to the original, translation into the ${to} language.
        For each word in the sentence, provide a full translation into ${to} language. Give several translation variants if necessary.
        Add a note on the use of the word if it's not clear how translation maps to the original.
        Preserve all punctuation, including all quotation marks and various kinds of parenthesis or braces.
        Put HTML-encoded values for punctuation signs in the 'original' field, e.g. comma turns into &comma;.
        For quotation marks, parenthesis, braces and similar signs fill out 'isOpeningParenthesis', 'isClosingParenthesis' correspondingly so the reader could you this information to reconstruct the original formatting.
        For punctuation signs which are meant to be written separately from words (e.g. em- and en-dashes) put 'true' in the 'isStandalonePunctuation' field. For punctuation signs which are written without space before it put 'false' into the 'isStandalonePunctuation' field.
        If you see an HTML line break (<br>) treat it as a standalone punctuation and preserve it in the output correspondingly.
        Provide grammatical information for each word. Grammatical information should ONLY be about the original word and how it's used in the original language. Do NOT use concepts from the target language when decribing the grammar. Use ONLY concepts which make sense and exist in the language of the original text, but use the ${to} language to describe it.
        All the information given must be in ${to} language except for the 'originalInitialForm', 'sourceLanguage' and 'targetLanguage' fields.
        Initial forms in the grammar section must be contain the form as it appears in the dictionaries in the language of the original and target text.
        'sourceLanguage' and 'targetLanguage' must contain ISO 639 Set 1 code of the corresponding language (e.g. 'en', 'de', 'ru', 'ja', etc.).
        Before giving the final answer to the user, re-read it and fix mistakes. Double-check that you correctly carried over the punctuation. Make sure that you don't accidentally use concepts which only exist in the ${to} language to describe word in the source text.
        Triple-check that you didn't miss any words!

        Input is given in JSON format, following this template:
        {
            paragraph: "string",
        }
        `
}