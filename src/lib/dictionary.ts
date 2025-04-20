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

export type Translation = {
    sentenceTranslation: string,
    original: string,
    translations: string[],
    note: string,
    grammar: Grammar,
};

const schema: Schema = {
    type: Type.OBJECT,
    properties: {
        "original": {
            type: Type.STRING,
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
            "properties": {
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
        "sentenceTranslation": {
            type: Type.STRING
        }
    },
    required: [
        "original",
        "translations",
        "grammar",
        "sentenceTranslation"
    ]
}

export type DictionaryRequest = {
    paragraph: string,
    sentence: string,
    word: {
        position: number,
        value: string,
    }
}

export class Dictionary {
    readonly book_hash: string
    readonly store: LocalForage
    readonly ai: GoogleGenAI;
    readonly to: string;

    private constructor(ai: GoogleGenAI, book_hash: string, to: string, store: LocalForage) {
        this.ai = ai;
        this.to = to;
        this.book_hash = book_hash;
        this.store = store;
    }

    static async build(ai: GoogleGenAI, book_hash: string, to: string) {
        const schemaHash = await hashString(JSON.stringify(schema));
        const storeName = await hashString(book_hash + to + schemaHash);
        const store = localforage.createInstance({
            storeName: storeName,
        })
        return new Dictionary(ai, book_hash, to, store);
    }

    async getCachedTranslation(p: DictionaryRequest) {
        const p_hash = await hashString(JSON.stringify(p));
        const translation = await this.store.getItem(p_hash) as Translation | null;
        return translation;
    }

    async getTranslation(p: DictionaryRequest) {
        const requestString = JSON.stringify(p);
        const p_hash = await hashString(requestString);
        const response = await this.ai.models.generateContent({
            model: "gemini-2.5-flash-preview-04-17",
            //model: "gemini-2.0-flash-lite",
            contents: requestString,
            config: {
                systemInstruction: this.getPrompt(this.to),
                responseMimeType: 'application/json',
                responseSchema: schema,
            }
        });
        const translation = JSON.parse(response.text!) as Translation;
        await this.store.setItem(p_hash, translation);

        return translation;
    }

    private getPrompt(to: string) {
        return `You are given a paragraph, a sentence and a word with its position numbered form 0.
        Provide original spelling of the word as given in the text.
        Provide grammatical information for the word. Provide a full translation of the sentence, keeping it as close to the original as possible.
        Provide a translation of this word into ${to} taking into account all the given context.
        Give several variants if necessary.
        Add a note on the use of the word if it's not clear how translation maps to the original.
        All the information given must be in ${to} language.
        Initial form in the grammar section must be contain the form as it appears in the dictionaries in the language of the original text.

        Input is given in JSON format, following this template:
        {
            paragraph: "string",
            sentence: "string",
            word: {
                position: number,
                value: "string"
            }
        }
        `
    }
}