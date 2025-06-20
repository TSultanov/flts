import { getConfig } from "../config";
import { db, generateUID, type UUID } from "./db";
import { queueDb, type TranslationRequest } from "./queueDb";
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
                .where("paragraphUid")
                .equals(paragraph.uid)
                .count() > 0;
            
            const hasRequest = await queueDb.directTranslationRequests
                .where("paragraphUid")
                .equals(paragraph.uid)
                .count() > 0;
            
            if (!hasTranslation && !hasRequest) {
                await library.scheduleTranslation(paragraph.uid);
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
const directTranslationRequestsQuery = liveQuery(async () => await queueDb.directTranslationRequests.limit(limit).toArray());
function scheduleTranslationWithRetries(request: TranslationRequest, retriesLeft = 5) {
    function schedule(retriesLeft: number) {
        translationRequestBag.add(request.id);
        queue.schedule(async () => {
            await handleTranslationEvent(request);
        }).then(() => {
            console.log(`Worker: paragraph uid ${request.paragraphUid} translation task is completed`);
            translationRequestBag.delete(request.id);
        })
            .catch((err) => {
                console.log(`Worker: error translating ${request.paragraphUid}, retrying (${retriesLeft - 1} attempts left)`, err);
                if (retriesLeft > 0) {
                    setTimeout(() => schedule(retriesLeft - 1), 300);
                } else {
                    console.log(`Failed to translate ${request.paragraphUid}`);
                    translationRequestBag.delete(request.id);
                }
            })
    }

    if (!translationRequestBag.has(request.id)) {
        console.log(`Worker: scheduling ${request.paragraphUid}`);
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
    console.log(`Worker: starting translation, paragraphUid: ${translationRequest.paragraphUid} (request ${translationRequest.id})`);

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
    const paragraph = await db.paragraphs.where('uid').equals(translationRequest.paragraphUid).first();
    console.log(`Worker: db.paragraphs.where took ${(performance.now() - stepStartTime).toFixed(2)}ms`);

    if (!paragraph) {
        console.log(`Worker: paragraph UID ${translationRequest.paragraphUid} does not exist`);
        await queueDb.directTranslationRequests.where("id").equals(translationRequest.id).delete()
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
    await addTranslation(translationRequest.paragraphUid, translation, translationRequest.model);
    console.log(`Worker: addTranslation took ${(performance.now() - stepStartTime).toFixed(2)}ms`);

    // Clean up request
    stepStartTime = performance.now();
    await queueDb.directTranslationRequests.where("id").equals(translationRequest.id).delete()
    console.log(`Worker: delete request took ${(performance.now() - stepStartTime).toFixed(2)}ms`);

    const totalTime = performance.now() - startTime;
    console.log(`Worker: handleTranslationEvent total time: ${totalTime.toFixed(2)}ms for paragraphUid ${translationRequest.paragraphUid}`);
}

async function addTranslation(paragraphUid: UUID, translation: ParagraphTranslation, model: ModelId) {
    const startTime = performance.now();
    console.log(`Worker: addTranslation starting for paragraphUid ${paragraphUid}, ${translation.sentences.length} sentences`);
    
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
            const paragraph = await db.paragraphs.where('uid').equals(paragraphUid).first();
            
            if (!paragraph) {
                console.log(`Worker: paragraph ${paragraphUid} was removed during while we were waiting for the LLM response. Skipping.`)
                return;
            }

            // Get or create source language
            const sourceLanguageUid = await (async (): Promise<UUID> => {
                const existingLanguage = await db.languages
                    .where("name").equals(translation.sourceLanguage.toLowerCase())
                    .first();

                if (existingLanguage) {
                    return existingLanguage.uid;
                }

                const uid = generateUID();
                await db.languages.add({ 
                    name: translation.sourceLanguage.toLowerCase(),
                    uid,
                    createdAt: Date.now(),
                });

                return uid;
            })();

            // Get or create target language
            const targetLanguageUid = await (async (): Promise<UUID> => {
                const existingLanguage = await db.languages
                    .where("name").equals(translation.targetLanguage.toLowerCase())
                    .first();

                if (existingLanguage) {
                    return existingLanguage.uid;
                }

                const uid = generateUID();
                await db.languages.add({ 
                    name: translation.targetLanguage.toLowerCase(),
                    uid,
                    createdAt: Date.now(),
                });

                return uid;
            })();

            // Check if paragraph translation already exists
            const existingParagraphTranslation = await db.paragraphTranslations
                .where("paragraphUid").equals(paragraphUid)
                .and(pt => pt.languageUid === targetLanguageUid).first();

            if (existingParagraphTranslation) {
                console.log(`Worker: paragraph ${paragraphUid} is already translated to ${targetLanguageUid}`);
                return;
            }

            // Create paragraph translation
            const paragraphTranslationUid = generateUID();
            await db.paragraphTranslations.add({
                paragraphUid: paragraph.uid,
                languageUid: targetLanguageUid,
                translatingModel: model,
                uid: paragraphTranslationUid,
                createdAt: Date.now(),
            });

            // Process sentences and words
            let sentenceOrder = 0;
            for (const sentence of translation.sentences) {
                const sentenceTranslationUid = generateUID();
                await db.sentenceTranslations.add({
                    paragraphTranslationUid,
                    order: sentenceOrder,
                    fullTranslation: sentence.fullTranslation,
                    uid: sentenceTranslationUid,
                    createdAt: Date.now(),
                });

                let wordOrder = 0;
                for (const word of sentence.words) {
                    if (word.isPunctuation) {
                        await db.sentenceWordTranslations.add({
                            order: wordOrder,
                            sentenceUid: sentenceTranslationUid,
                            isPunctuation: word.isPunctuation,
                            isStandalonePunctuation: word.isStandalonePunctuation,
                            isOpeningParenthesis: word.isOpeningParenthesis,
                            isClosingParenthesis: word.isClosingParenthesis,
                            original: word.original,
                            uid: generateUID(),
                            createdAt: Date.now(),
                        })
                    } else {
                        const originalWordUid = await (async (): Promise<UUID> => {
                            const dictWord = await db.words
                            .where("originalNormalized").equals(word.grammar.originalInitialForm.toLowerCase())
                            .and(w => w.originalLanguageUid == sourceLanguageUid).first();

                            if (dictWord) {
                                return dictWord.uid;
                            }

                            const uid = generateUID();
                            await db.words.add({
                                originalLanguageUid: sourceLanguageUid,
                                original: word.grammar.originalInitialForm,
                                originalNormalized: word.grammar.originalInitialForm.toLowerCase(),
                                uid,
                                createdAt: Date.now(),
                            });

                            return uid;
                        })();

                        const wordTranslationUid = await (async (): Promise<UUID> => {
                            const existingTranslation = await db.wordTranslations
                            .where("originalWordUid").equals(originalWordUid)
                            .and(wt => wt.translationNormalized === word.grammar.targetInitialForm.toLowerCase())
                            .and(wt => wt.languageUid == targetLanguageUid).first();

                            if (existingTranslation) {
                                return existingTranslation.uid;
                            }

                            const uid = generateUID();
                            await db.wordTranslations.add({
                                languageUid: targetLanguageUid,
                                originalWordUid,
                                translation: word.grammar.targetInitialForm,
                                translationNormalized: word.grammar.targetInitialForm.toLowerCase(),
                                uid,
                                createdAt: Date.now(),
                            });

                            return uid;
                        })();

                        await db.sentenceWordTranslations.add({
                            order: wordOrder,
                            original: word.original,
                            isPunctuation: word.isPunctuation,
                            isStandalonePunctuation: word.isStandalonePunctuation,
                            isOpeningParenthesis: word.isOpeningParenthesis,
                            isClosingParenthesis: word.isClosingParenthesis,
                            sentenceUid: sentenceTranslationUid,
                            wordTranslationUid: wordTranslationUid,
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
    console.log(`Worker: addTranslation total time: ${totalTime.toFixed(2)}ms for paragraphUid ${paragraphUid}`);
}