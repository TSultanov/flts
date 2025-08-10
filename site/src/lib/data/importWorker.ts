import { getConfig } from "../config";
import { translationQueue, type TranslationRequest } from "./queueDb";
import Bottleneck from 'bottleneck';
import { liveQuery } from "dexie";
import { getTranslator, type ModelId, type ParagraphTranslation } from "./translators/translator";
import { books, type BookParagraphTranslation, type IBook, type ParagraphId, type SentenceTranslation, type SentenceWordTranslation } from "./v2/book.svelte";
import { dictionary } from "./v2/dictionary";

const limit = 1;

const queue = new Bottleneck({
    maxConcurrent: limit,
});

const translationSavingQueue = new Bottleneck({
    maxConcurrent: 1,
});

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

        const allBooks = await books.listBooks();

        let untranslatedCount = 0;

        for (const bookMeta of allBooks) {
            if (bookMeta.translationRatio >= 1.0) {
                continue;
            }

            const book = await books.getBook(bookMeta.uid);
            if (!book) {
                continue;
            }
            for (const chapter of book.chapters) {
                for (const paragraph of chapter.paragraphs) {
                    if (!paragraph.translation) {
                        if (!await translationQueue.hasRequest(book.uid, paragraph.id)) {
                            await translationQueue.scheduleTranslation(book.uid, paragraph.id);
                            untranslatedCount++;
                        }
                    }
                }
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
const directTranslationRequestsQuery = liveQuery(async () => await translationQueue.top(limit));
function scheduleTranslationWithRetries(request: TranslationRequest, retriesLeft = 5) {
    function schedule(retriesLeft: number) {
        translationRequestBag.add(request.id);
        queue.schedule(async () => {
            await handleTranslationEvent(request);
        }).then(() => {
            console.log(`Worker: book uid ${request.bookUid} paragraph id ${request.paragraphId.chapter}/${request.paragraphId.paragraph} translation task is completed`);
            translationRequestBag.delete(request.id);
        })
            .catch((err) => {
                console.log(`Worker: error translating book uid ${request.bookUid} paragraph id ${request.paragraphId.chapter}/${request.paragraphId.paragraph}, retrying (${retriesLeft - 1} attempts left)`, err);
                if (retriesLeft > 0) {
                    setTimeout(() => schedule(retriesLeft - 1), 300);
                } else {
                    console.log(`Failed to translate book uid ${request.bookUid} paragraph id ${request.paragraphId.chapter}/${request.paragraphId.paragraph}`);
                    translationRequestBag.delete(request.id);
                }
            })
    }

    if (!translationRequestBag.has(request.id)) {
        console.log(`Worker: scheduling book uid ${request.bookUid} paragraph id ${request.paragraphId.chapter}/${request.paragraphId.paragraph}`);
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
    console.log(`Worker: starting translation,book uid ${translationRequest.bookUid} paragraph id ${translationRequest.paragraphId.chapter}/${translationRequest.paragraphId.paragraph}(request ${translationRequest.id})`);

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
    const book = await books.getBook(translationRequest.bookUid);

    if (!book) {
        console.log(`Worker: book UID ${translationRequest.bookUid} does not exist`);
        await translationQueue.removeRequest(translationRequest.id);
        return;
    }

    const paragraph = book.getParagraphView(translationRequest.paragraphId);

    if (!paragraph) {
        console.log(`Worker: book UID ${translationRequest.bookUid} paragraph Id ${translationRequest.paragraphId.chapter}/${translationRequest.paragraphId.paragraph} does not exist`);
        await translationQueue.removeRequest(translationRequest.id);
        return;
    }

    const request = {
        paragraph: paragraph.originalPlain
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
        await addTranslation(book, translationRequest.paragraphId, translation, translationRequest.model);
        console.log(`Worker: addTranslation took ${(performance.now() - stepStartTime).toFixed(2)}ms`);

        // Clean up request
        stepStartTime = performance.now();
        await translationQueue.removeRequest(translationRequest.id);
        console.log(`Worker: delete request took ${(performance.now() - stepStartTime).toFixed(2)}ms`);

        const totalTime = performance.now() - startTime;
        console.log(`Worker: handleTranslationEvent total time: ${totalTime.toFixed(2)}ms for book UID ${translationRequest.bookUid} paragraph Id ${translationRequest.paragraphId.chapter}/${translationRequest.paragraphId.paragraph}`);
    });
}

export async function addTranslation(book: IBook, paragraphId: ParagraphId, translation: ParagraphTranslation, model: ModelId) {
    const startTime = performance.now();
    console.log(`Worker: addTranslation starting for bookUid ${book.uid} paragraphId ${paragraphId.chapter}/${paragraphId.paragraph}, ${translation.sentences.length} sentences`);

    const pTranslations: BookParagraphTranslation = {
        languageCode: translation.targetLanguage,
        translatingModel: model,
        sentences: []
    };

    for (const s of translation.sentences) {
        const sentenceTranslation: SentenceTranslation = {
            fullTranslation: s.fullTranslation,
            words: []
        };

        for (const w of s.words) {
            const wordTranslation: SentenceWordTranslation = {
                original: w.original,
                isPunctuation: w.isPunctuation,
                isStandalonePunctuation: w.isStandalonePunctuation,
                isOpeningParenthesis: w.isOpeningParenthesis,
                isClosingParenthesis: w.isClosingParenthesis,
                wordTranslationUid: await dictionary.addTranslation(
                    w.grammar.originalInitialForm,
                    translation.sourceLanguage,
                    w.grammar.targetInitialForm,
                    translation.targetLanguage,
                ),
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

    book.updateParagraphTranslation(paragraphId, pTranslations);

    const totalTime = performance.now() - startTime;
    console.log(`Worker: addTranslation total time: ${totalTime.toFixed(2)}ms for bookUid ${book.uid} paragraphId ${paragraphId.chapter}/${paragraphId.paragraph}`);
}