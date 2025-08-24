import { getConfig } from "../config";
import { translationQueue, type TranslationRequest } from "./queueDb";
import Bottleneck from 'bottleneck';
import { liveQuery } from "dexie";
import { getTranslator, type ModelId, type ParagraphTranslation } from "./translators/translator";
import { dictionary } from "./sql/dictionary"
import { sqlBooks, type IBookMeta, type UpdateParagraphTranslationMessageSentence, type UpdateParagraphTranslationMessageTranslation } from "./sql/book";
import type { UUID } from "./v2/db";
import { readableToPromise } from "./sql/utils";

const limit = 1;

const queue = new Bottleneck({
    maxConcurrent: limit,
});

const translationSavingQueue = new Bottleneck({
    maxConcurrent: 1,
});

// Function to check all paragraphs and schedule translation for untranslated ones
export async function checkAndScheduleUntranslatedParagraphs(allBooks: IBookMeta[]) {
    try {
        console.log('Worker: Checking for untranslated paragraphs...');

        const config = await getConfig();
        const targetLanguage = config.targetLanguage;

        if (!targetLanguage) {
            console.log('Worker: No target language configured, skipping untranslated paragraph check');
            return;
        }

        let untranslatedCount = 0;

        for (const bookMeta of allBooks) {
            if (bookMeta.translationRatio >= 1.0) {
                continue;
            }


            const untranslatedParagraphs = await readableToPromise(sqlBooks.getNotTranslatedParagraphsUids(bookMeta.uid));
            if (!untranslatedParagraphs) {
                continue;
            }
            for (const p of untranslatedParagraphs) {
                if (await translationQueue.hasRequest(bookMeta.uid, p)) continue;
                await translationQueue.scheduleTranslation(bookMeta.uid, p);
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

export async function startTranslations() {
    sqlBooks.listBooks().subscribe(books => {
        checkAndScheduleUntranslatedParagraphs(books);
    });

    const translationRequestBag: Set<number> = new Set();
    const directTranslationRequestsQuery = liveQuery(async () => await translationQueue.top(limit));
    function scheduleTranslationWithRetries(request: TranslationRequest, retriesLeft = 5) {
        function schedule(retriesLeft: number) {
            translationRequestBag.add(request.id);
            queue.schedule(async () => {
                await handleTranslationEvent(request);
            }).then(() => {
                console.log(`Worker: book uid ${request.bookUid} paragraph id ${request.paragraphUid} translation task is completed`);
                translationRequestBag.delete(request.id);
            })
                .catch((err) => {
                    console.log(`Worker: error translating book uid ${request.bookUid} paragraph id ${request.paragraphUid} retrying (${retriesLeft - 1} attempts left)`, err);
                    if (retriesLeft > 0) {
                        setTimeout(() => schedule(retriesLeft - 1), 300);
                    } else {
                        console.log(`Failed to translate book uid ${request.bookUid} paragraph id ${request.paragraphUid}`);
                        translationRequestBag.delete(request.id);
                    }
                })
        }

        if (!translationRequestBag.has(request.id)) {
            console.log(`Worker: scheduling book uid ${request.bookUid} paragraph id ${request.paragraphUid}`);
            schedule(retriesLeft);
        }
    }

    directTranslationRequestsQuery.subscribe((requests: TranslationRequest[]) => {
        for (const request of requests) {
            scheduleTranslationWithRetries(request);
        }
    })
}

async function handleTranslationEvent(translationRequest: TranslationRequest) {
    const startTime = performance.now();
    console.log(`Worker: starting translation, book uid ${translationRequest.bookUid} paragraph id ${translationRequest.paragraphUid} (request ${translationRequest.id})`);

    // Get config
    let stepStartTime = performance.now();
    const config = await getConfig();
    console.log(`Worker: getConfig took ${(performance.now() - stepStartTime).toFixed(2)}ms`);

    // Get translator
    stepStartTime = performance.now();
    const translator = await getTranslator(config.targetLanguage, translationRequest.model);
    console.log(`Worker: getTranslator took ${(performance.now() - stepStartTime).toFixed(2)}ms`);

    // Get paragraph from database
    stepStartTime = performance.now();

    const paragraph = await readableToPromise(sqlBooks.getParagraph(translationRequest.paragraphUid));
    if (!paragraph) {
        console.log(`Worker: paragraph with UID ${translationRequest.paragraphUid} does not exist`);
        await translationQueue.removeRequest(translationRequest.id);
        return;
    }
    console.log(`Worker: paragraph id ${translationRequest.paragraphUid}: ${paragraph.originalText.substring(0, 20)}...`)

    if (!paragraph) {
        console.log(`Worker: book UID ${translationRequest.bookUid} paragraph Id ${translationRequest.paragraphUid} does not exist`);
        await translationQueue.removeRequest(translationRequest.id);
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

    await translationSavingQueue.schedule(async () => {
        // Add translation to database
        stepStartTime = performance.now();
        await addTranslation(translationRequest.paragraphUid, translation, translationRequest.model);
        console.log(`Worker: addTranslation took ${(performance.now() - stepStartTime).toFixed(2)}ms`);

        // Clean up request
        stepStartTime = performance.now();
        await translationQueue.removeRequest(translationRequest.id);
        console.log(`Worker: delete request took ${(performance.now() - stepStartTime).toFixed(2)}ms`);

        const totalTime = performance.now() - startTime;
        console.log(`Worker: handleTranslationEvent total time: ${totalTime.toFixed(2)}ms for book UID ${translationRequest.bookUid} paragraph Id ${translationRequest.paragraphUid}`);
    });
}

export async function addTranslation(paragraphUid: UUID, translation: ParagraphTranslation, model: ModelId) {
    const startTime = performance.now();
    console.log(`Worker: addTranslation starting for paragraphId ${paragraphUid}, ${translation.sentences.length} sentences`);

    const targetLanguageUid = await dictionary.getLanguageUidByCode(translation.targetLanguage);

    const pTranslations: UpdateParagraphTranslationMessageTranslation = {
        languageUid: targetLanguageUid,
        translatingModel: model,
        sentences: []
    };

    // Metrics for dictionary.addTranslation performance
    let dictAddTotalTime = 0;
    let dictAddCalls = 0;

    for (const s of translation.sentences) {
        const sentenceTranslation: UpdateParagraphTranslationMessageSentence = {
            fullTranslation: s.fullTranslation,
            words: []
        };

        for (const w of s.words) {
            const dictStart = performance.now();
            const wordTranslationUid = await dictionary.addTranslation({
                originalWord: w.grammar.originalInitialForm,
                originalLanguageCode: translation.sourceLanguage,
                targetWord: w.grammar.targetInitialForm,
                targetLanguageCode: translation.targetLanguage
            },
            );
            dictAddTotalTime += performance.now() - dictStart;
            dictAddCalls++;

            const wordTranslation = {
                original: w.original,
                isPunctuation: w.isPunctuation,
                isStandalonePunctuation: w.isStandalonePunctuation,
                isOpeningParenthesis: w.isOpeningParenthesis,
                isClosingParenthesis: w.isClosingParenthesis,
                wordTranslationUid: wordTranslationUid,
                wordTranslationInContext: w.translations,
                grammarContext: {
                    partOfSpeech: w.grammar.partOfSpeech,
                    originalInitialForm: w.grammar.originalInitialForm,
                    targetInitialForm: w.grammar.targetInitialForm,
                    plurality: w.grammar.plurality,
                    person: w.grammar.person,
                    tense: w.grammar.tense,
                    case: w.grammar.case,
                    other: w.grammar.other,
                },
                note: w.note,
            }
            sentenceTranslation.words.push(wordTranslation);
        }

        pTranslations.sentences.push(sentenceTranslation);
    }

    await sqlBooks.updateParagraphTranslation({
        paragraphUid,
        translation: pTranslations
    });

    if (dictAddCalls > 0) {
        console.log(
            `Worker: addTranslation dictionary.addTranslation cumulative: ${dictAddTotalTime.toFixed(2)}ms over ${dictAddCalls} calls (avg ${(dictAddTotalTime / dictAddCalls).toFixed(2)}ms)`
        );
    } else {
        console.log(`Worker: addTranslation dictionary.addTranslation had no calls (no words processed)`);
    }

    const totalTime = performance.now() - startTime;
    console.log(`Worker: addTranslation total time: ${totalTime.toFixed(2)}ms for paragraphId ${paragraphUid}`);
}