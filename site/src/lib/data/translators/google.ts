import localforage from "localforage";
import { hashString } from "../utils";
import { GoogleGenAI, Type, type Schema } from "@google/genai";
import type { DB } from "../db";
import type { DictionaryRequest, ParagraphTranslation, Translator } from "./translator";

const wordSchema: Schema = {
    type: Type.OBJECT,
    properties: {
        "original": {
            type: Type.STRING,
        },
        "isPunctuation": {
            type: Type.BOOLEAN,
        },
        "isStandalonePunctuation": {
            type: Type.BOOLEAN,
        },
        "isOpeningParenthesis": {
            type: Type.BOOLEAN,
        },
        "isClosingParenthesis": {
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
                "originalInitialForm": {
                    type: Type.STRING
                },
                "targetInitialForm": {
                    type: Type.STRING
                },
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
                }
            },
            required: [
                "partOfSpeech",
                "originalInitialForm",
                "targetInitialForm"
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
        },
        "fullTranslation": {
            type: Type.STRING,
        }
    },
    required: [
        "words",
        "fullTranslation"
    ]
}

const paragraphSchema: Schema = {
    type: Type.OBJECT,
    properties: {
        "sentences": {
            type: Type.ARRAY,
            items: sentenceSchema
        },
        "sourceLanguage": {
            type: Type.STRING
        },
        "targetLanguage": {
            type: Type.STRING
        }
    },
    required: [
        "sentences",
        "sourceLanguage",
        "targetLanguage"
    ]
}

export class GoogleTranslator implements Translator {
    readonly db: DB
    readonly ai: GoogleGenAI;
    readonly to: string;
    readonly model: string;

    constructor(apiKey: string, to: string, db: DB, model: string) {
        this.ai = new GoogleGenAI({ apiKey });
        this.to = to;
        this.db = db;
        this.model = model;
    }

    private async hashRequest(p: DictionaryRequest) {
        return await hashString(JSON.stringify(p) + this.getPrompt() + JSON.stringify(paragraphSchema) + this.model);
    }

    async getCachedTranslation(p: DictionaryRequest): Promise<ParagraphTranslation | null> {
        const p_hash = await this.hashRequest(p);
        const cacheItem = await this.db.queryCache.get(p_hash);
        if (cacheItem) {
            return cacheItem.value as ParagraphTranslation;
        }
        return null;
    }

    private async setCachedTranslation(p_hash: string, t: ParagraphTranslation) {
        await this.db.queryCache.put({
            hash: p_hash,
            value: t
        });
    }

    async getTranslation(p: DictionaryRequest): Promise<ParagraphTranslation> {
        const requestString = JSON.stringify(p);
        const p_hash = await this.hashRequest(p);
        const response = await this.ai.models.generateContent({
            model: this.model,
            contents: requestString,
            config: {
                systemInstruction: this.getPrompt(),
                responseMimeType: 'application/json',
                responseSchema: paragraphSchema,
            }
        });
        const translation = JSON.parse(response.text!) as ParagraphTranslation;
        await this.setCachedTranslation(p_hash, translation);

        return translation;
    }

    private getPrompt() {
        return `You are given a paragraph in a foreign language. The goal is to construct a translation which can be used by somebody who speaks the ${this.to} language to learn the original language.
        For each sentence provide a good, but close to the original, translation into the ${this.to} language.
        For each word in the sentence, provide a full translation into ${this.to} language. Give several translation variants if necessary.
        Add a note on the use of the word if it's not clear how translation maps to the original.
        Preserve all punctuation, including all quotation marks and various kinds of parenthesis or braces.
        Put HTML-encoded values for punctuation signs in the 'original' field, e.g. comma turns into &comma;.
        For quotation marks, parenthesis, braces and similar signs fill out 'isOpeningParenthesis', 'isClosingParenthesis' correspondingly so the reader could you this information to reconstruct the original formatting.
        For punctuation signs which are meant to be written separately from words (e.g. em- and en-dashes) put 'true' in the 'isStandalonePunctuation' field. For punctuation signs which are written without space before it put 'false' into the 'isStandalonePunctuation' field.
        If you see an HTML line break (<br>) treat it as a standalone punctuation and preserve it in the output correspondingly.
        Provide grammatical information for each word. Grammatical information should ONLY be about the original word and how it's used in the original language. Do NOT use concepts from the target language when decribing the grammar. Use ONLY concepts which make sense and exist in the language of the original text, but use the ${this.to} language to describe it.
        All the information given must be in ${this.to} language except for the 'originalInitialForm', 'sourceLanguage' and 'targetLanguage' fields.
        Initial forms in the grammar section must be contain the form as it appears in the dictionaries in the language of the original and target text.
        'sourceLanguage' and 'targetLanguage' must contain ISO 639 Set 1 code of the corresponding language.
        Before giving the final answer to the user, re-read it and fix mistakes. Double-check that you correctly carried over the punctuation. Make sure that you don't accidentally use concepts which only exist in the ${this.to} language to describe word in the source text.
        Triple-check that you didn't miss any words!

        Input is given in JSON format, following this template:
        {
            paragraph: "string",
        }
        `
    }
}