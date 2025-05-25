import { GoogleGenAI } from "@google/genai"
import { getConfig } from "../config";
import { Translator } from "./translator";
import { db } from "./db";

const RETRY_INTERNAL = 5000;

type MessageType = 'ParagraphTranslationRequest' | 'ScheduleTranslationRequest';

interface Request {
    __brand: MessageType
}

interface ParagraphTranslationRequest extends Request {
    __brand: 'ParagraphTranslationRequest',
    paragraphId: number,
    targetLanguage: string
}

interface ScheduleTranslationRequest extends Request {
    __brand: 'ScheduleTranslationRequest';
}

function reschedule(e: any) {
    setInterval(() => {
        self.postMessage(e)
    }, RETRY_INTERNAL);
}

export function startScheduling() {
    self.postMessage({ __brand: 'ScheduleTranslationRequest' })
}

onmessage = async (e: MessageEvent<ParagraphTranslationRequest | ScheduleTranslationRequest>) => {
    const config = await getConfig();
    if (!config.apiKey || config.apiKey.length === 0) {
        console.log("Worker: apiKey is not set in config");
        reschedule(e);
    }

    if (!config.targetLanguage || config.targetLanguage.length === 0) {
        console.log("Worker: targetLanguage is not set in config");
        reschedule(e);
    }


    switch (e.data?.__brand) {
        case 'ParagraphTranslationRequest': {
            await handleParagraphTranslationEvent(e.data);
            break;
        }
        case 'ScheduleTranslationRequest': {
            await scheduleTranslation();
            break;
        }
    }
}

async function scheduleTranslation() {

}

async function handleParagraphTranslationEvent(e: ParagraphTranslationRequest) {
    const config = await getConfig();
    const ai = new GoogleGenAI({ apiKey: config.apiKey });
    const translator = new Translator(ai, e.targetLanguage, db);

    console.log(`Worker: starting translation, paragraphId: ${e.paragraphId}`);

    const paragraph = await db.paragraphs.get(e.paragraphId);

    if (!paragraph) {
        console.log(`Worker: paragraph ${e.paragraphId} not found in the database, skipping.`);
        startScheduling();
        return;
    }

    const request = {
        paragraph: paragraph.originalText
    };

    let translation = await translator.getCachedTranslation(request);
    if (!translation) {
        translation = await translator.getTranslation(request);
    }

    db.transaction(
        'rw',
        [
            db.languages,
            db.paragraphTranslations,
            db.sentenceTranslations,
            db.sentenceWordTranslations,
            db.words,
            db.wordTranslations,
        ],
        async () => {
            const sourceLanguageId = await (async () => {
                let id = (await db.languages
                    .filter((l) => l.name.toLowerCase() === translation.sourceLanguage.toLowerCase())
                    .first())?.id;

                if (!id) {
                    id = await db.languages.add({ name: translation.sourceLanguage.toLowerCase() });
                }

                return id;
            })();

            const targetLanguageId = await (async () => {
                let id = (await db.languages
                    .filter((l) => l.name.toLowerCase() === translation.targetLanguage.toLowerCase())
                    .first())?.id;

                if (!id) {
                    id = await db.languages.add({ name: translation.targetLanguage.toLowerCase() });
                }

                return id;
            })();

            const paragraphTranslationId = await db.paragraphTranslations.add({
                paragraphId: e.paragraphId,
                languageId: targetLanguageId,
            });

            let sentenceOrder = 0;
            for (const sentence of translation.sentences) {
                const sentenceTranslationId = await db.sentenceTranslations.add({
                    paragraphTranslationId,
                    order: sentenceOrder,
                });

                let wordOrder = 0;
                for (const word of sentence.words) {
                    if (word.isPunctuation) {
                        await db.sentenceWordTranslations.add({
                            order: wordOrder,
                            sentenceId: sentenceTranslationId,
                            isPunctuation: word.isPunctuation,
                            original: word.original
                        })
                    } else {

                        const originalWordId = await (async () => {
                            const dictWord = await db.words.filter(w => {
                                return w.originalLanguageId === sourceLanguageId &&
                                    w.original.toLowerCase() === word.grammar.originalInitialForm.toLowerCase();
                            }).first();

                            let id = dictWord?.id;
                            if (!id) {
                                id = await db.words.add({
                                    originalLanguageId: sourceLanguageId,
                                    original: word.grammar.originalInitialForm,
                                });
                            }

                            return id;
                        })();

                        const wordTranslationId = await (async () => {
                            const translation = await db.wordTranslations.filter(wt => {
                                return wt.languageId === targetLanguageId &&
                                    wt.originalWordId === originalWordId &&
                                    wt.translation.toLowerCase() === word.grammar.translationInitialForm.toLowerCase();
                            }).first();

                            let id = translation?.id;

                            if (!id) {
                                id = await db.wordTranslations.add({
                                    languageId: targetLanguageId,
                                    originalWordId,
                                    translation: word.grammar.translationInitialForm
                                });
                            }

                            return id;
                        })();
                        
                        await db.sentenceWordTranslations.add({
                            order: wordOrder,
                            original: word.original,
                            isPunctuation: word.isPunctuation,
                            sentenceId: sentenceTranslationId,
                            wordTranslationId: wordTranslationId,
                            wordTranslationInContext: word.translations,
                            grammarContext: word.grammar,
                            note: word.note,
                        })
                    }

                    wordOrder += 1;
                }
                sentenceOrder += 1;
            }

        }).then(() => {
            console.log(`Worker: paragraph ${e.paragraphId} translation saved`);
        }).catch(err => {
            console.log("Worker: failed to save translation:", err);
        }).finally(() => {
            startScheduling();
        });
}

