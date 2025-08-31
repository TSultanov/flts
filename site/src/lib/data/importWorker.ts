import { getConfig } from "../config";
import { TranslationQueue, type TranslationRequest } from "./queueDb";
import Bottleneck from 'bottleneck';
import { liveQuery } from "dexie";
import { getTranslator, type ModelId, type ParagraphTranslation } from "./translators/translator";
import type { 
    Books,
    ParagraphTranslation as DBParagraphTranslation,
    ParagraphTranslationSentence as DBParagraphTranslationSentence,
    ParagraphTranslationSentenceWord as DBParagraphTranslationSentenceWord,
    ParagraphTranslationSentenceWordGrammar
} from "./evolu/book";
import type { Evolu } from "@evolu/common";
import type { BookChapterParagraphId, DatabaseSchema } from "./evolu/schema";
import type { Dictionary } from "./evolu/dictionary";

const limit = 1;

const queue = new Bottleneck({
    maxConcurrent: limit,
});

const translationSavingQueue = new Bottleneck({
    maxConcurrent: 1,
});

export class TranslationWorker {
    constructor(
        private evolu: Evolu<DatabaseSchema>,
        private books: Books,
        private dictionary: Dictionary,
        private translationQueue: TranslationQueue,
    ) {}

    // Function to check all paragraphs and schedule translation for untranslated ones
    async checkAndScheduleUntranslatedParagraphs() {
        try {
            console.log('Worker: Checking for untranslated paragraphs...');

            const config = await getConfig();
            const targetLanguage = config.targetLanguage;

            if (!targetLanguage) {
                console.log('Worker: No target language configured, skipping untranslated paragraph check');
                return;
            }

            let untranslatedCount = 0;


            const untranslatedParagraphs = await this.evolu.loadQuery(this.books.nonTranslatedParagraphsIds());

            for (const p of untranslatedParagraphs) {
                if (await this.translationQueue.hasRequest(p.id)) continue;
                await this.translationQueue.scheduleTranslation(p.id);
                untranslatedCount++;
            }

            if (untranslatedCount > 0) {
                console.log(`Worker: Found ${untranslatedCount} untranslated paragraphs, scheduled for translation`);
            } else {
                console.log('Worker: No untranslated paragraphs found');
            }

        } catch (error) {
            console.error('Worker: Error checking untranslated paragraphs:', error);
        }
    }

    async startTranslations() {
        this.checkAndScheduleUntranslatedParagraphs();

        const translationRequestBag: Set<number> = new Set();
        const directTranslationRequestsQuery = liveQuery(async () => await this.translationQueue.top(limit));

        const scheduleTranslationWithRetries = (request: TranslationRequest, retriesLeft = 5) => {
            const schedule = (retriesLeft: number) => {
                translationRequestBag.add(request.id);
                queue.schedule(async () => {
                    await this.handleTranslationEvent(request);
                }).then(() => {
                    console.log(`Worker: paragraph id ${request.paragraphId} translation task is completed`);
                    translationRequestBag.delete(request.id);
                })
                    .catch((err) => {
                        console.log(`Worker: error translating paragraph id ${request.paragraphId} retrying (${retriesLeft - 1} attempts left)`, err);
                        if (retriesLeft > 0) {
                            setTimeout(() => schedule(retriesLeft - 1), 300);
                        } else {
                            console.log(`Failed to translate paragraph id ${request.paragraphId}`);
                            translationRequestBag.delete(request.id);
                        }
                    })
            }

            if (!translationRequestBag.has(request.id)) {
                console.log(`Worker: scheduling paragraph id ${request.paragraphId}`);
                schedule(retriesLeft);
            }
        }

        directTranslationRequestsQuery.subscribe((requests: TranslationRequest[]) => {
            for (const request of requests) {
                scheduleTranslationWithRetries(request);
            }
        })
    }

    async handleTranslationEvent(translationRequest: TranslationRequest) {
        const startTime = performance.now();
        console.log(`Worker: starting translation, paragraph id ${translationRequest.paragraphId} (request ${translationRequest.id})`);

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

        const paragraphsResult = await this.evolu.loadQuery(this.books.paragraph(translationRequest.paragraphId));

        if (paragraphsResult.length === 0) {
            console.log(`Worker: paragraph with UID ${translationRequest.paragraphId} does not exist`);
            await this.translationQueue.removeRequest(translationRequest.id);
            return;
        }

        const paragraph = paragraphsResult[0];

        if (!paragraph.originalText) {
            console.log(`Worker: paragraph with UID ${translationRequest.paragraphId} is malformed`);
            await this.translationQueue.removeRequest(translationRequest.id);
            return;
        }

        console.log(`Worker: paragraph id ${translationRequest.paragraphId}: ${paragraph.originalText.substring(0, 20)}...`)

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
            await this.addTranslation(translationRequest.paragraphId, translation, translationRequest.model);
            console.log(`Worker: addTranslation took ${(performance.now() - stepStartTime).toFixed(2)}ms`);

            // Clean up request
            stepStartTime = performance.now();
            await this.translationQueue.removeRequest(translationRequest.id);
            console.log(`Worker: delete request took ${(performance.now() - stepStartTime).toFixed(2)}ms`);

            const totalTime = performance.now() - startTime;
            console.log(`Worker: handleTranslationEvent total time: ${totalTime.toFixed(2)}ms for paragraph Id ${translationRequest.paragraphId}`);
        });
    }

    async addTranslation(paragraphId: BookChapterParagraphId, translation: ParagraphTranslation, model: ModelId) {
        const startTime = performance.now();
        console.log(`Worker: addTranslation starting for paragraphId ${paragraphId}, ${translation.sentences.length} sentences`);

        const targetLanguageId = this.dictionary.upsertLanguage(translation.targetLanguage);

        const pTranslations: DBParagraphTranslation = {
            languageId: targetLanguageId,
            translatingModel: model,
            sentences: []
        };

        // Metrics for dictionary.addTranslation performance
        let dictAddTotalTime = 0;
        let dictAddCalls = 0;

        for (const s of translation.sentences) {
            const sentenceTranslation: DBParagraphTranslationSentence = {
                fullTranslation: s.fullTranslation,
                words: []
            };

            for (const w of s.words) {
                const dictStart = performance.now();
                const wordTranslationId = await this.dictionary.addTranslation(
                    w.grammar.originalInitialForm,
                    translation.sourceLanguage,
                    w.grammar.targetInitialForm,
                    translation.targetLanguage
                );
                dictAddTotalTime += performance.now() - dictStart;
                dictAddCalls++;

                const grammarContext: ParagraphTranslationSentenceWordGrammar = {
                        partOfSpeech: w.grammar.partOfSpeech,
                        originalInitialForm: w.grammar.originalInitialForm,
                        targetInitialForm: w.grammar.targetInitialForm,
                        plurality: w.grammar.plurality,
                        person: w.grammar.person,
                        tense: w.grammar.tense,
                        case: w.grammar.case,
                        other: w.grammar.other,
                    };

                const wordTranslation: DBParagraphTranslationSentenceWord = {
                    original: w.original,
                    isPunctuation: w.isPunctuation,
                    isStandalonePunctuation: w.isStandalonePunctuation,
                    isOpeningParenthesis: w.isOpeningParenthesis,
                    isClosingParenthesis: w.isClosingParenthesis,
                    wordTranslationId: wordTranslationId,
                    wordTranslationInContext: w.translations,
                    grammarContext: grammarContext,
                    note: w.note,
                }
                sentenceTranslation.words.push(wordTranslation);
            }

            pTranslations.sentences.push(sentenceTranslation);
        }

        this.books.updateParagraphTranslation(paragraphId, pTranslations);

        if (dictAddCalls > 0) {
            console.log(
                `Worker: addTranslation dictionary.addTranslation cumulative: ${dictAddTotalTime.toFixed(2)}ms over ${dictAddCalls} calls (avg ${(dictAddTotalTime / dictAddCalls).toFixed(2)}ms)`
            );
        } else {
            console.log(`Worker: addTranslation dictionary.addTranslation had no calls (no words processed)`);
        }

        const totalTime = performance.now() - startTime;
        console.log(`Worker: addTranslation total time: ${totalTime.toFixed(2)}ms for paragraphId ${paragraphId}`);
    }
}