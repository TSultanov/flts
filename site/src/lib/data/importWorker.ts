import { getConfig } from "../config";
import { queueDb, type TranslationRequest } from "./queueDb";
import Bottleneck from 'bottleneck';
import { liveQuery } from "dexie";
import { getTranslator } from "./translators/translator";
import dbSql from "./dbSql";

const limit = 10;

const queue = new Bottleneck({
    maxConcurrent: limit,
});

// Create library instance for reusing translation scheduling logic
const library = dbSql.getLibrary();

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

        const unstranslatedParagraphUids = await dbSql.getUntranslatedParagraphs();
        
        let untranslatedCount = 0;
        
        for (const paragraphUid of unstranslatedParagraphUids) {
            const hasRequest = await queueDb.directTranslationRequests
                .where("paragraphUid")
                .equals(paragraphUid)
                .count() > 0;
            
            if (!hasRequest) {
                await library.scheduleTranslation(paragraphUid);
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
    const translator = await getTranslator(config.targetLanguage, translationRequest.model);
    console.log(`Worker: getTranslator took ${(performance.now() - stepStartTime).toFixed(2)}ms`);

    // Get paragraph from database
    stepStartTime = performance.now();
    const paragraph = await dbSql.getParagraph(translationRequest.paragraphUid);
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

    // Add translation to database using SQL
    stepStartTime = performance.now();
    await dbSql.addTranslation(translationRequest.paragraphUid, translation, translationRequest.model);
    console.log(`Worker: addTranslation took ${(performance.now() - stepStartTime).toFixed(2)}ms`);

    // Clean up request
    stepStartTime = performance.now();
    await queueDb.directTranslationRequests.where("id").equals(translationRequest.id).delete()
    console.log(`Worker: delete request took ${(performance.now() - stepStartTime).toFixed(2)}ms`);

    const totalTime = performance.now() - startTime;
    console.log(`Worker: handleTranslationEvent total time: ${totalTime.toFixed(2)}ms for paragraphUid ${translationRequest.paragraphUid}`);
}

