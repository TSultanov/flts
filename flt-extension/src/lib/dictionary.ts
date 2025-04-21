import { storage } from '#imports';
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
    readonly ai: GoogleGenAI;
    readonly to: string;

    constructor(ai: GoogleGenAI, to: string) {
        this.ai = ai;
        this.to = to;
    }

    async getCachedTranslation(p: DictionaryRequest) {
        const requestString = JSON.stringify(p);
        const p_hash = await this.hashRequest(requestString);
        const translation = await storage.getItem(`local:${p_hash}`) as Translation | null;
        return translation;
    }

    async getTranslation(p: DictionaryRequest) {
        const requestString = JSON.stringify(p);
        const p_hash = await this.hashRequest(requestString);
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
        await storage.setItem(`local:${p_hash}`, translation);

        return translation;
    }

    private async hashRequest(requestString: string) {
        const schemaHash = await hashString(JSON.stringify(schema));
        return await hashString(requestString + this.to + schemaHash);
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