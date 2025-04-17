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
    translations: string[],
    note: string,
    grammar: Grammar,
};

export type SentenceTranslation = {
    fullTranslation: string,
    words: Array<WordTranslation>,
};

export type ParagraphTranslation = {
    sentences: Array<SentenceTranslation>,
};

const schema: Schema = {
    type: Type.OBJECT,
    properties: {
        "sentences": {
            type: Type.ARRAY,
            items: {
                type: Type.OBJECT,
                properties: {
                    "fullTranslation": {
                        type: Type.STRING
                    },
                    "words": {
                        type: Type.ARRAY,
                        items: {
                            type: Type.OBJECT,
                            properties: {
                                "original": {
                                    type: Type.STRING
                                },
                                "translations": {
                                    type: Type.ARRAY,
                                    "items": {
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
                                        "plurality": {
                                            type: Type.STRING
                                        },
                                        "person": {
                                            type: Type.STRING
                                        },
                                        "tense": {
                                            type: Type.STRING
                                        },
                                        "case": {
                                            type: Type.STRING
                                        },
                                        "other": {
                                            type: Type.STRING
                                        }
                                    },
                                    "required": [
                                        "partOfSpeech"
                                    ]
                                }
                            },
                            "required": [
                                "original",
                                "translations"
                            ]
                        }
                    }
                },
                "required": [
                    "fullTranslation",
                    "words"
                ]
            }
        }
    },
    "required": [
        "sentences"
    ]
};

export class Dictionary {
    readonly book_hash: string
    readonly store: LocalForage
    readonly ai: GoogleGenAI;
    readonly from: string;
    readonly to: string;

    private constructor(ai: GoogleGenAI, book_hash: string, from: string, to: string, store: LocalForage) {
        this.ai = ai;
        this.from = from;
        this.to = to;
        this.book_hash = book_hash;
        this.store = store;
    }

    static async build(ai: GoogleGenAI, book_hash: string, from: string, to: string) {
        const schemaHash = await hashString(JSON.stringify(schema));
        const storeName = await hashString(book_hash + from + to + schemaHash);
        const store = localforage.createInstance({
            storeName: storeName,
        })
        return new Dictionary(ai, book_hash, from ,to, store);
    }

    async translateParagraph(p: string) {
        const p_hash = await hashString(p);
        let translation = await this.store.getItem(p_hash) as ParagraphTranslation;

        if (!translation) {
            const response = await this.ai.models.generateContent({
                //model: "gemini-2.5-flash-preview-04-17",
                model: "gemini-2.0-flash-lite",
                contents: p,
                config: {
                    systemInstruction: `You are given text in ${this.from} language. Provide first a full ${this.to} translation of each sentence, and then a per-word translation of it into ${this.to}. Add several variants of translation for each word. Add note on the use of ech word if it's not clear how the translation maps to the original. Add grammatical information for each original word. Spell all notes and grammatical remarks in the target lagnuage. Skip punctuation.`,
                    responseMimeType: 'application/json',
                    responseSchema: schema,
                }
            });
            translation = JSON.parse(response.text!) as ParagraphTranslation;
            await this.store.setItem(p_hash, translation);
        }

        return translation;
    }
}