import localforage from "localforage";
import { hashString } from "./utils";
import { Type, type GoogleGenAI, type Schema } from "@google/genai";

type Grammar = {
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
    translations: string[],
    note: string,
    grammar: Grammar,
};

export type SentenceTranslation = {
    words: WordTranslation[],
}

export type ParagraphTranslation = {
    sentences: SentenceTranslation[],
}

const wordSchema: Schema = {
    type: Type.OBJECT,
    properties: {
        "original": {
            type: Type.STRING,
        },
        "isPunctuation": {
            type: Type.BOOLEAN,
        },
        "translations": {
            type: Type.ARRAY,
            items: {
                type: Type.STRING
            }
        },
        "note": {
            type: Type.STRING
        },
        "grammar": {
            type: Type.OBJECT,
            properties: {
                "partOfSpeech": {
                    type: Type.STRING
                },
                "tense": {
                    type: Type.STRING
                },
                "person": {
                    type: Type.STRING
                },
                "case": {
                    type: Type.STRING
                },
                "plurality": {
                    type: Type.STRING
                },
                "other": {
                    type: Type.STRING
                },
                "initialForm": {
                    type: Type.STRING
                }
            },
            required: [
                "partOfSpeech",
                "initialForm"
            ]
        },
    },
    required: [
        "original",
        "translations",
        "grammar"
    ]
}

const sentenceSchema: Schema = {
    type: Type.OBJECT,
    properties: {
        "words": {
            type: Type.ARRAY,
            items: wordSchema
        }
    }
}

const paragraphSchema: Schema = {
    type: Type.OBJECT,
    properties: {
        "sentences": {
            type: Type.ARRAY,
            items: sentenceSchema
        }
    }
}

export type DictionaryRequest = {
    paragraph: string,
}

export class Translator {
    readonly store: LocalForage
    readonly ai: GoogleGenAI;
    readonly to: string;

    private constructor(ai: GoogleGenAI, to: string, store: LocalForage) {
        this.ai = ai;
        this.to = to;
        this.store = store;
    }

    static async build(ai: GoogleGenAI, to: string) {
        const schemaHash = await hashString(JSON.stringify(paragraphSchema)+Translator.getPrompt(to));
        const storeName = `${to}_${schemaHash}`;
        const store = localforage.createInstance({
            storeName: storeName,
        })
        return new Translator(ai, to, store);
    }

    async getCachedTranslation(p: DictionaryRequest) {
        const p_hash = await hashString(JSON.stringify(p));
        const translation = await this.store.getItem(p_hash) as ParagraphTranslation | null;
        return translation;
    }

    async getTranslation(p: DictionaryRequest) {
        const requestString = JSON.stringify(p);
        const p_hash = await hashString(requestString);
        const response = await this.ai.models.generateContent({
            model: "gemini-2.5-flash-preview-04-17",
            contents: requestString,
            config: {
                systemInstruction: Translator.getPrompt(this.to),
                responseMimeType: 'application/json',
                responseSchema: paragraphSchema,
            }
        });
        const translation = JSON.parse(response.text!) as ParagraphTranslation;
        await this.store.setItem(p_hash, translation);

        return translation;
    }

    private static getPrompt(to: string) {
        return `You are given a paragraph in a foreign language.
        Provide a full translation of each word in the paragraph into ${to} language, grouping them in sentences.
        Preserve all punctuation. Put HTML-encoded values for punctuation signs in the "original" field, e.g. comma turns into &comma;.
        Provide grammatical information for each word.
        Give several translation variants if necessary.
        Add a note on the use of the word if it's not clear how translation maps to the original.
        All the information given must be in ${to} language.
        Initial form in the grammar section must be contain the form as it appears in the dictionaries in the language of the original text.

        Input is given in JSON format, following this template:
        {
            paragraph: "string",
        }
        `
    }
}