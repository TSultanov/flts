import { hashString } from "../utils";
import { GoogleGenAI, Type, type Schema } from "@google/genai";
import { getCached, setCache } from "../cache";
import { getPrompt, type DictionaryRequest, type ModelId, type ParagraphTranslation, type Translator } from "./translator";

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
        "note",
        "grammar",
        "isPunctuation"
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
    readonly ai: GoogleGenAI;
    readonly to: string;
    readonly model: ModelId;

    constructor(apiKey: string, to: string, model: ModelId) {
        this.ai = new GoogleGenAI({ apiKey });
        this.to = to;
        this.model = model;
    }

    private async hashRequest(p: DictionaryRequest) {
        return await hashString(JSON.stringify(p) + getPrompt(this.to) + JSON.stringify(paragraphSchema) + this.model);
    }

    async getCachedTranslation(p: DictionaryRequest): Promise<ParagraphTranslation | null> {
        const p_hash = await this.hashRequest(p);
        const cachedValue = await getCached<ParagraphTranslation>(p_hash);
        return cachedValue;
    }

    private async setCachedTranslation(p_hash: string, t: ParagraphTranslation) {
        await setCache(p_hash, t);
    }

    async getTranslation(p: DictionaryRequest): Promise<ParagraphTranslation> {
        const requestString = JSON.stringify(p);
        const p_hash = await this.hashRequest(p);
        const response = await this.ai.models.generateContent({
            model: this.model,
            contents: requestString,
            config: {
                systemInstruction: getPrompt(this.to),
                responseMimeType: 'application/json',
                responseSchema: paragraphSchema,
            }
        });
        const translation = JSON.parse(response.text!) as ParagraphTranslation;
        await this.setCachedTranslation(p_hash, translation);

        return translation;
    }
}