import { getConfig } from "../config";
import { db, type TranslationRequest, generateUID } from "./db";
import Bottleneck from 'bottleneck';
import { liveQuery } from "dexie";
import { getTranslator, type ModelId, type ParagraphTranslation } from "./translators/translator";
import { Library } from "../library.svelte";

const limit = 10;

const queue = new Bottleneck({
    maxConcurrent: limit,
});

// Create library instance for reusing translation scheduling logic
const library = new Library();

// Function to check all paragraphs and schedule translation for untranslated ones
async function checkAndScheduleUntranslatedParagraphs() {
    try {
        console.log('Worker: Checking for untranslated paragraphs...');
        
        const config = await getConfig();
        const targetLanguage = config.targetLanguage;
        
        if (!targetLanguage) {
            console.log('Worker: No target language configured, skipping untranslated paragraph check');
            return;
        }

        const allParagraphs = await db.paragraphs.toArray();
        
        let untranslatedCount = 0;
        
        for (const paragraph of allParagraphs) {
            let hasTranslation = false;

            hasTranslation = await db.paragraphTranslations
                .where("paragraphId")
                .equals(paragraph.id)
                .count() > 0;
            
            const hasRequest = await db.directTranslationRequests
                .where("paragraphId")
                .equals(paragraph.id)
                .count() > 0;
            
            if (!hasTranslation && !hasRequest) {
                await library.scheduleTranslation(paragraph.id);
                untranslatedCount++;
            }
        }
        
        if (untranslatedCount > 0) {
            console.log(`Worker: Found ${untranslatedCount} untranslated paragraphs, scheduled for translation`);
        } else {
            console.log('Worker: No unstranslated paragraphs found');
        }
        
    } catch (error) {
        console.error('Worker: Error checking untranslated paragraphs:', error);
    }
}

checkAndScheduleUntranslatedParagraphs();

const translationRequestBag: Set<number> = new Set();
const directTranslationRequestsQuery = liveQuery(async () => await db.directTranslationRequests.limit(limit).toArray());
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
        schedule(retriesLeft);
    }
}
directTranslationRequestsQuery.subscribe((requests: TranslationRequest[]) => {
    for (const request of requests) {
        scheduleTranslationWithRetries(request);
    }
})

async function handleTranslationEvent(translationRequest: TranslationRequest) {
    const startTime = performance.now();
    console.log(`Worker: starting translation, paragraphId: ${translationRequest.paragraphId} (request ${translationRequest.id})`);

    // Get config
    let stepStartTime = performance.now();
    const config = await getConfig();
    console.log(`Worker: getConfig took ${(performance.now() - stepStartTime).toFixed(2)}ms`);

    // Get translator
    stepStartTime = performance.now();
    const translator = await getTranslator(db, config.targetLanguage, translationRequest.model);
    console.log(`Worker: getTranslator took ${(performance.now() - stepStartTime).toFixed(2)}ms`);

    // Get paragraph from database
    stepStartTime = performance.now();
    const paragraph = await db.paragraphs.get(translationRequest.paragraphId);
    console.log(`Worker: db.paragraphs.get took ${(performance.now() - stepStartTime).toFixed(2)}ms`);

    if (!paragraph) {
        console.log(`Worker: paragraph Id ${translationRequest.paragraphId} does not exist`);
        await db.directTranslationRequests.where("id").equals(translationRequest.id).delete()
        return;
    }

    const request = {
        paragraph: paragraph.originalText
    };

    // Check cached translation
    stepStartTime = performance.now();
    let translation = await translator.getCachedTranslation(request);
    console.log(`Worker: getCachedTranslation took ${(performance.now() - stepStartTime).toFixed(2)}ms`);
    
    if (!translation) {
        // Get new translation
        stepStartTime = performance.now();
        translation = await translator.getTranslation(request);
        console.log(`Worker: getTranslation took ${(performance.now() - stepStartTime).toFixed(2)}ms`);
    }

    // Add translation to database
    stepStartTime = performance.now();
    await addTranslation(translationRequest.paragraphId, translation, translationRequest.model);
    console.log(`Worker: addTranslation took ${(performance.now() - stepStartTime).toFixed(2)}ms`);

    // Clean up request
    stepStartTime = performance.now();
    await db.directTranslationRequests.where("id").equals(translationRequest.id).delete()
    console.log(`Worker: delete request took ${(performance.now() - stepStartTime).toFixed(2)}ms`);

    const totalTime = performance.now() - startTime;
    console.log(`Worker: handleTranslationEvent total time: ${totalTime.toFixed(2)}ms for paragraphId ${translationRequest.paragraphId}`);
}

async function addTranslation(paragraphId: number, translation: ParagraphTranslation, model: ModelId) {
    const startTime = performance.now();
    console.log(`Worker: addTranslation starting for paragraphId ${paragraphId}, ${translation.sentences.length} sentences`);
    
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
            const paragraphCount = await db.paragraphs.where("id").equals(paragraphId).count();
            
            if (paragraphCount === 0) {
                console.log(`Worker: paragraph ${paragraphId} was removed during while we were waiting for the LLM response. Skipping.`)
                return;
            }

            // Get or create source language
            const sourceLanguageId = await (async () => {
                let id = (await db.languages
                    .where("name").equals(translation.sourceLanguage.toLowerCase())
                    .first())?.id;

                if (!id) {
                    id = await db.languages.add({ 
                        name: translation.sourceLanguage.toLowerCase(),
                        uid: generateUID(),
                        createdAt: Date.now(),
                    });
                }

                return id;
            })();

            // Get or create target language
            const targetLanguageId = await (async () => {
                let id = (await db.languages
                    .where("name").equals(translation.targetLanguage.toLowerCase())
                    .first())?.id;

                if (!id) {
                    id = await db.languages.add({ 
                        name: translation.targetLanguage.toLowerCase(),
                        uid: generateUID(),
                        createdAt: Date.now(),
                    });
                }

                return id;
            })();

            // Check if paragraph translation already exists
            const existingParagraphTranslation = await db.paragraphTranslations
                .where("paragraphId").equals(paragraphId)
                .and(pt => pt.languageId === targetLanguageId).first();

            if (existingParagraphTranslation) {
                console.log(`Worker: paragraph ${paragraphId} is already translated to ${targetLanguageId} (id ${existingParagraphTranslation.id})`);
                return;
            }

            // Create paragraph translation
            const paragraphTranslationId = await db.paragraphTranslations.add({
                paragraphId: paragraphId,
                languageId: targetLanguageId,
                translatingModel: model,
                uid: generateUID(),
                createdAt: Date.now(),
            });

            // Process sentences and words
            let sentenceOrder = 0;
            for (const sentence of translation.sentences) {
                let sentenceStepStart = performance.now();
                const sentenceTranslationId = await db.sentenceTranslations.add({
                    paragraphTranslationId,
                    order: sentenceOrder,
                    fullTranslation: sentence.fullTranslation,
                    uid: generateUID(),
                    createdAt: Date.now(),
                });

                let wordOrder = 0;
                for (const word of sentence.words) {
                    if (word.isPunctuation) {
                        sentenceStepStart = performance.now();
                        await db.sentenceWordTranslations.add({
                            order: wordOrder,
                            sentenceId: sentenceTranslationId,
                            isPunctuation: word.isPunctuation,
                            isStandalonePunctuation: word.isStandalonePunctuation,
                            isOpeningParenthesis: word.isOpeningParenthesis,
                            isClosingParenthesis: word.isClosingParenthesis,
                            original: word.original,
                            uid: generateUID(),
                            createdAt: Date.now(),
                        })
                    } else {
                        sentenceStepStart = performance.now();
                        const originalWordId = await (async () => {
                            const dictWord = await db.words
                            .where("originalNormalized").equals(word.grammar.originalInitialForm.toLowerCase())
                            .and(w => w.originalLanguageId == sourceLanguageId).first();

                            let id = dictWord?.id;
                            if (!id) {
                                id = await db.words.add({
                                    originalLanguageId: sourceLanguageId,
                                    original: word.grammar.originalInitialForm,
                                    originalNormalized: word.grammar.originalInitialForm.toLowerCase(),
                                    uid: generateUID(),
                                    createdAt: Date.now(),
                                });
                            }

                            return id;
                        })();

                        sentenceStepStart = performance.now();
                        const wordTranslationId = await (async () => {
                            const translation = await db.wordTranslations
                            .where("originalWordId").equals(originalWordId)
                            .and(wt => wt.translationNormalized === word.grammar.targetInitialForm.toLowerCase())
                            .and(wt => wt.languageId == targetLanguageId).first();

                            let id = translation?.id;

                            if (!id) {
                                id = await db.wordTranslations.add({
                                    languageId: targetLanguageId,
                                    originalWordId,
                                    translation: word.grammar.targetInitialForm,
                                    translationNormalized: word.grammar.targetInitialForm.toLowerCase(),
                                    uid: generateUID(),
                                    createdAt: Date.now(),
                                });
                            }

                            return id;
                        })();

                        sentenceStepStart = performance.now();
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
                            uid: generateUID(),
                            createdAt: Date.now(),
                        })
                    }

                    wordOrder += 1;
                }
                sentenceOrder += 1;
            }
        });
    
    const totalTime = performance.now() - startTime;
    console.log(`Worker: addTranslation total time: ${totalTime.toFixed(2)}ms for paragraphId ${paragraphId}`);
}