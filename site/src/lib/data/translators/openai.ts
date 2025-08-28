import { z } from "zod";
import { getPrompt, type DictionaryRequest, type ModelId, type ParagraphTranslation, type Translator } from "./translator";
import OpenAI from "openai";
import { hashString } from "../utils";
import { getCached, setCache } from "../cache";
import { zodTextFormat } from "openai/helpers/zod";

const wordSchema = z.object({
    original: z.string(),
    isPunctuation: z.boolean(),
    isStandalonePunctuation: z.boolean().nullable(),
    isOpeningParenthesis: z.boolean().nullable(),
    isClosingParenthesis: z.boolean().nullable(),
    translations: z.array(z.string()),
    note: z.string(),
    grammar: z.object({
        originalInitialForm: z.string(),
        targetInitialForm: z.string(),
        partOfSpeech: z.string(),
        tense: z.string().nullable(),
        person: z.string().nullable(),
        case: z.string().nullable(),
        plurality: z.string().nullable(),
        other: z.string().nullable()
    })
});

const sentenceSchema = z.object({
    words: z.array(wordSchema),
    fullTranslation: z.string()
});

const schema = z.object({
    sentences: z.array(sentenceSchema),
    sourceLanguage: z.string(),
    targetLanguage: z.string()
});

const CalendarEvent = z.object({
  name: z.string(),
  date: z.string(),
  participants: z.array(z.string()),
});

export class OpenAITranslator implements Translator {
    private readonly openai: OpenAI;
    constructor(apiKey: string, private readonly to: string, private readonly model: ModelId) {
        this.openai = new OpenAI({
            apiKey
        });
    }

    private async hashRequest(p: DictionaryRequest) {
        return await hashString(JSON.stringify(p) + getPrompt(this.to) + JSON.stringify(schema) + this.model);
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

        const format = zodTextFormat(schema, "translation");

        const response = await this.openai.responses.parse({
            model: this.model,
            input: [
                { role: 'system', content: getPrompt(this.to) },
                { role: 'user', content: requestString }
            ],
            text: {
                format
            }
        });

        const translation = response.output_parsed!;
        await this.setCachedTranslation(p_hash, translation);

        return translation
    }   
}