import { GoogleGenAI } from "@google/genai"
import { getConfig } from "../config";
import { Translator } from "./translator";
import { db } from "./db";
import Bottleneck from 'bottleneck';
import { liveQuery } from "dexie";

const RETRY_INTERNAL = 5000;

const queue = new Bottleneck({
    maxConcurrent: 10,
});
let paragraphTranslationBag: Set<number> = new Set();

function reschedule(e: any) {
    setInterval(() => {
        self.postMessage(e)
    }, RETRY_INTERNAL);
}

function startScheduling() {
    self.postMessage({ __brand: 'ScheduleTranslationRequest' })
}

const query = liveQuery(async () => await db.transaction(
    'r',
    [
        db.paragraphTranslations,
        db.paragraphs,
    ],
    async () => {
        const translatedParagraphIds = (await db.paragraphTranslations.toArray()).map(x => x.paragraphId);
        const notTranslatedParagraphs = (await db.paragraphs.where("id").noneOf(translatedParagraphIds).toArray()).map(x => x.id);
        return notTranslatedParagraphs;
    }
));

function scheduleTranslationWithRetries(id: number, retriesLeft = 5) {
    function schedule(retriesLeft: number) {
        paragraphTranslationBag.add(id);
        queue.schedule(async () => {
            await handleParagraphTranslationEvent(id);
        }).then(() => {
            console.log(`Worker: paragraph id ${id} translation task is completed`);
            paragraphTranslationBag.delete(id);
        })
        .catch((err) => {
            console.log(`Worker: error translating ${id}, retrying (${retriesLeft - 1} attempts left)`, err);
            if (retriesLeft > 0) {
                setTimeout(() => schedule(retriesLeft - 1), 300);
            } else {
                console.log(`Failed to translate ${id}`);
                paragraphTranslationBag.delete(id);
            }
        })
    }

    if (!paragraphTranslationBag.has(id)) {
        console.log(`Worker: scheduling ${id}`);
        schedule(retriesLeft);
    }
}

// We don't handle `unsubscribe` because the subject will be destroyed when the web worker terminates anyway.
query.subscribe((ids: number[]) => {
    for (const id of ids) {
        scheduleTranslationWithRetries(id);
    }
});

async function handleParagraphTranslationEvent(paragraphId: number) {
    const config = await getConfig();
    const ai = new GoogleGenAI({ apiKey: config.apiKey });
    const translator = new Translator(ai, config.targetLanguage, db);

    console.log(`Worker: starting translation, paragraphId: ${paragraphId}`);

    const paragraph = await db.paragraphs.get(paragraphId);

    if (!paragraph) {
        console.log(`Worker: paragraph Id ${paragraphId} does not exist`);
        return;
    }

    const request = {
        paragraph: paragraph.originalText
    };

    let translation = await translator.getCachedTranslation(request);
    if (!translation) {
        translation = await translator.getTranslation(request);
    }

    await db.transaction(
        'rw',
        [
            db.languages,
            db.paragraphs,
            db.paragraphTranslations,
            db.sentenceTranslations,
            db.sentenceWordTranslations,
            db.words,
            db.wordTranslations,
        ],
        async () => {
            // check if paragraph indeed exists and was not removed while we waited for the LLM response
            const paragraph = await db.paragraphs.get(paragraphId);
            if (!paragraph) {
                console.log(`Worker: paragraph ${paragraphId} was removed during while we were waiting for the LLM response. Skipping.`)
                return;
            }

            const sourceLanguageId = await (async () => {
                let id = (await db.languages
                    .filter((l) => l.name?.toLowerCase() === translation.sourceLanguage.toLowerCase())
                    .first())?.id;

                if (!id) {
                    id = await db.languages.add({ name: translation.sourceLanguage.toLowerCase() });
                }

                return id;
            })();

            const targetLanguageId = await (async () => {
                let id = (await db.languages
                    .filter((l) => l.name?.toLowerCase() === translation.targetLanguage.toLowerCase())
                    .first())?.id;

                if (!id) {
                    id = await db.languages.add({ name: translation.targetLanguage.toLowerCase() });
                }

                return id;
            })();

            // Check if paragraph translation already exists
            const existingParagraphTranslation = await db.paragraphTranslations
                .where("paragraphId")
                .equals(paragraphId).and(pt => pt.languageId === targetLanguageId).first();

            if (existingParagraphTranslation) {
                console.log(`Worker: paragraph ${paragraphId} is already translated to ${targetLanguageId} (id ${existingParagraphTranslation.id})`);
                return;
            }

            const paragraphTranslationId = await db.paragraphTranslations.add({
                paragraphId: paragraphId,
                languageId: targetLanguageId,
            });

            let sentenceOrder = 0;
            for (const sentence of translation.sentences) {
                const sentenceTranslationId = await db.sentenceTranslations.add({
                    paragraphTranslationId,
                    order: sentenceOrder,
                    fullTranslation: sentence.fullTranslation,
                });

                let wordOrder = 0;
                for (const word of sentence.words) {
                    if (word.isPunctuation) {
                        await db.sentenceWordTranslations.add({
                            order: wordOrder,
                            sentenceId: sentenceTranslationId,
                            isPunctuation: word.isPunctuation,
                            isStandalonePunctuation: word.isStandalonePunctuation,
                            isOpeningParenthesis: word.isOpeningParenthesis,
                            isClosingParenthesis: word.isClosingParenthesis,
                            original: word.original
                        })
                    } else {

                        const originalWordId = await (async () => {
                            const dictWord = await db.words.filter(w => {
                                return w.originalLanguageId === sourceLanguageId &&
                                    w.original?.toLowerCase() === word.grammar.originalInitialForm.toLowerCase();
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
                                    wt.translation?.toLowerCase() === word.grammar.targetInitialForm.toLowerCase();
                            }).first();

                            let id = translation?.id;

                            if (!id) {
                                id = await db.wordTranslations.add({
                                    languageId: targetLanguageId,
                                    originalWordId,
                                    translation: word.grammar.targetInitialForm
                                });
                            }

                            return id;
                        })();

                        await db.sentenceWordTranslations.add({
                            order: wordOrder,
                            original: word.original,
                            isPunctuation: word.isPunctuation,
                            isStandalonePunctuation: word.isStandalonePunctuation,
                            isOpeningParenthesis: word.isOpeningParenthesis,
                            isClosingParenthesis: word.isClosingParenthesis,
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

        });
}

