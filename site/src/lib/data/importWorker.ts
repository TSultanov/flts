import { getConfig } from "../config";
import { GoogleTranslator } from "./translators/google";
import { db, type TranslationRequest } from "./db";
import Bottleneck from 'bottleneck';
import { liveQuery } from "dexie";
import { getTranslator, type ModelId, type ParagraphTranslation } from "./translators/translator";

const RETRY_INTERNAL = 5000;

const queue = new Bottleneck({
    maxConcurrent: 10,
});

function reschedule(e: any) {
    setInterval(() => {
        self.postMessage(e)
    }, RETRY_INTERNAL);
}

function startScheduling() {
    self.postMessage({ __brand: 'ScheduleTranslationRequest' })
}

const translationRequestBag: Set<number> = new Set();
const directTranslationRequestsQuery = liveQuery(async () => await db.directTranslationRequests.toArray());
function scheduleTranslationWithRetries(request: TranslationRequest, retriesLeft = 5) {
    function schedule(retriesLeft: number) {
        translationRequestBag.add(request.id);
        queue.schedule(async () => {
            await handleTranslationEvent(request);
        }).then(() => {
            console.log(`Worker: paragraph id ${request.paragraphId} translation task is completed`);
            translationRequestBag.delete(request.id);
        })
            .catch((err) => {
                console.log(`Worker: error translating ${request.paragraphId}, retrying (${retriesLeft - 1} attempts left)`, err);
                if (retriesLeft > 0) {
                    setTimeout(() => schedule(retriesLeft - 1), 300);
                } else {
                    console.log(`Failed to translate ${request.paragraphId}`);
                    translationRequestBag.delete(request.id);
                }
            })
    }

    if (!translationRequestBag.has(request.id)) {
        console.log(`Worker: scheduling ${request.paragraphId}`);
        // Delete the request from the table immediately when scheduled
        db.directTranslationRequests.where("id").equals(request.id).delete()
            .then(() => {
                console.log(`Worker: removed translation request ${request.id} from table`);
            })
            .catch((err) => {
                console.log(`Worker: failed to remove translation request ${request.id} from table:`, err);
            });
        schedule(retriesLeft);
    }
}
directTranslationRequestsQuery.subscribe((requests: TranslationRequest[]) => {
    for (const request of requests) {
        scheduleTranslationWithRetries(request);
    }
})

async function handleTranslationEvent(translationRequest: TranslationRequest) {
    const config = await getConfig();
    const translator = await getTranslator(db, config.targetLanguage, translationRequest.model);

    console.log(`Worker: starting translation, paragraphId: ${translationRequest.paragraphId}`);

    const paragraph = await db.paragraphs.get(translationRequest.paragraphId);

    if (!paragraph) {
        console.log(`Worker: paragraph Id ${translationRequest.paragraphId} does not exist`);
        return;
    }

    const request = {
        paragraph: paragraph.originalText
    };

    let translation = await translator.getCachedTranslation(request);
    if (!translation) {
        translation = await translator.getTranslation(request);
    }

    await addTranslation(translationRequest.paragraphId, translation, translationRequest.model);
}

async function addTranslation(paragraphId: number, translation: ParagraphTranslation, model: ModelId) {
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
                translatingModel: model,
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