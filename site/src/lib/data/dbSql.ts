import { PGliteWorker } from '@electric-sql/pglite/worker'
import Worker from "./dbWorker?worker"
import { applyMigrations } from './sql/migrations';
import { live, type PGliteWithLive } from '@electric-sql/pglite/live'
import { type UUID, type Paragraph, generateUID } from './db';
import type { ParagraphTranslation, ModelId } from './translators/translator';
import { Library } from '../library.svelte';
import { PGlite, type PGliteInterface } from '@electric-sql/pglite';

let pg: PGliteInterface;
// Use in-memory PGlite when running in Vitest, PGliteWorker otherwise
if (import.meta.vitest || !globalThis.Worker) {
    // In Vitest or when Worker is not available: use in-memory PGlite
    pg = new PGlite('', {
        extensions: {
            live
        }
    });
} else {
    // In production with Worker support: use PGliteWorker for better performance
    pg = new PGliteWorker(new Worker(), {
        extensions: {
            live
        }
    });
}

await applyMigrations(pg);

function getLibrary() {
    return new Library(pg as unknown as PGliteWithLive);
}

/**
 * Reset the database by dropping all tables and reapplying migrations
 * This is useful for testing
 */
async function resetDatabase() {
    // Drop all tables
    const tables = [
        'sentence_word_translations',
        'word_translations', 
        'sentence_translations',
        'paragraph_translations',
        'paragraphs',
        'book_chapters',
        'books',
        'words',
        'languages',
        'migrations'
    ];
    
    for (const table of tables) {
        await pg.query(`DROP TABLE IF EXISTS ${table} CASCADE`);
    }
    
    // Reapply migrations
    await applyMigrations(pg);
}

/**
 * Get the pg instance for testing purposes
 */
function getPgInstance() {
    return pg;
}

/**
 * Get all paragraphs that don't have translations in the SQL database
 * This is equivalent to the Dexie-based logic in checkAndScheduleUntranslatedParagraphs
 * Note: Caller still needs to check queueDb for pending translation requests separately
 */
async function getUntranslatedParagraphs(): Promise<UUID[]> {
    const result = await pg.query(`
        SELECT DISTINCT p.uid
        FROM paragraphs p
        LEFT JOIN paragraph_translations pt ON p.uid = pt.paragraph_uid
        WHERE pt.uid IS NULL
    `);
    
    return result.rows.map((row: any) => (row.uid as UUID));
}

/**
 * Get a paragraph by its UID
 * @param uid - The UUID of the paragraph to retrieve
 * @returns The Paragraph object or null if it doesn't exist
 */
async function getParagraph(uid: UUID): Promise<Paragraph | null> {
    const result = await pg.query(`
        SELECT uid, created_at, chapter_uid, "order", original_text, original_html
        FROM paragraphs
        WHERE uid = $1
    `, [uid]);
    
    if (result.rows.length === 0) {
        return null;
    }
    
    const row = result.rows[0] as any;
    return {
        uid: row.uid as UUID,
        createdAt: row.created_at as number,
        chapterUid: row.chapter_uid as UUID,
        order: row.order as number,
        originalText: row.original_text as string,
        originalHtml: row.original_html as string | undefined,
    };
}

/**
 * Add a translation for a paragraph using SQL with auto-generated UUIDs
 * @param paragraphUid - The UUID of the paragraph to add translation for
 * @param translation - The paragraph translation data
 * @param model - The model ID used for translation
 */
async function addTranslation(paragraphUid: UUID, translation: ParagraphTranslation, model: ModelId): Promise<void> {
    const startTime = performance.now();
    console.log(`SQL: addTranslation starting for paragraphUid ${paragraphUid}, ${translation.sentences.length} sentences`);
    
    await pg.transaction(async (tx) => {
        // Check if paragraph exists
        const paragraphResult = await tx.query(`
            SELECT uid FROM paragraphs WHERE uid = $1
        `, [paragraphUid]);
        
        if (paragraphResult.rows.length === 0) {
            console.log(`SQL: paragraph ${paragraphUid} was removed while we were waiting for the LLM response. Skipping.`);
            return;
        }

        // Get or create source language
        const sourceLanguageUid = await (async (): Promise<UUID> => {
            const result = await tx.query(`
                SELECT uid FROM languages WHERE name = $1
            `, [translation.sourceLanguage.toLowerCase()]);

            if (result.rows.length > 0) {
                return (result.rows[0] as any).uid as UUID;
            }

            const insertResult = await tx.query(`
                INSERT INTO languages (uid, name, created_at)
                VALUES ($1, $2, $3)
                RETURNING uid
            `, [generateUID(), translation.sourceLanguage.toLowerCase(), Date.now()]);

            return (insertResult.rows[0] as any).uid as UUID;
        })();

        // Get or create target language
        const targetLanguageUid = await (async (): Promise<UUID> => {
            const result = await tx.query(`
                SELECT uid FROM languages WHERE name = $1
            `, [translation.targetLanguage.toLowerCase()]);

            if (result.rows.length > 0) {
                return (result.rows[0] as any).uid as UUID;
            }

            const insertResult = await tx.query(`
                INSERT INTO languages (uid, name, created_at)
                VALUES ($1, $2, $3)
                RETURNING uid
            `, [generateUID(), translation.targetLanguage.toLowerCase(), Date.now()]);

            return (insertResult.rows[0] as any).uid as UUID;
        })();

        // Check if paragraph translation already exists
        const existingResult = await tx.query(`
            SELECT uid FROM paragraph_translations 
            WHERE paragraph_uid = $1 AND language_uid = $2
        `, [paragraphUid, targetLanguageUid]);

        if (existingResult.rows.length > 0) {
            console.log(`SQL: paragraph ${paragraphUid} is already translated to ${targetLanguageUid}`);
            return;
        }

        // Create paragraph translation
        const paragraphTranslationResult = await tx.query(`
            INSERT INTO paragraph_translations (uid, paragraph_uid, language_uid, translating_model, created_at)
            VALUES ($1, $2, $3, $4, $5)
            RETURNING uid
        `, [generateUID(), paragraphUid, targetLanguageUid, model, Date.now()]);
        
        const paragraphTranslationUid = (paragraphTranslationResult.rows[0] as any).uid as UUID;

        // Process sentences and words
        let sentenceOrder = 0;
        for (const sentence of translation.sentences) {
            const sentenceResult = await tx.query(`
                INSERT INTO sentence_translations (uid, paragraph_translation_uid, "order", full_translation, created_at)
                VALUES ($1, $2, $3, $4, $5)
                RETURNING uid
            `, [generateUID(), paragraphTranslationUid, sentenceOrder, sentence.fullTranslation, Date.now()]);
            
            const sentenceTranslationUid = (sentenceResult.rows[0] as any).uid as UUID;

            let wordOrder = 0;
            for (const word of sentence.words) {
                if (word.isPunctuation) {
                    await tx.query(`
                        INSERT INTO sentence_word_translations (
                            uid, sentence_uid, "order", original, is_punctuation, 
                            is_standalone_punctuation, is_opening_parenthesis, is_closing_parenthesis, 
                            created_at
                        )
                        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                    `, [
                        generateUID(),
                        sentenceTranslationUid,
                        wordOrder,
                        word.original,
                        word.isPunctuation,
                        word.isStandalonePunctuation ?? false,
                        word.isOpeningParenthesis ?? false,
                        word.isClosingParenthesis ?? false,
                        Date.now()
                    ]);
                } else {
                    // Get or create original word
                    const originalWordUid = await (async (): Promise<UUID> => {
                        const result = await tx.query(`
                            SELECT uid FROM words 
                            WHERE original_normalized = $1 AND original_language_uid = $2
                        `, [word.grammar.originalInitialForm.toLowerCase(), sourceLanguageUid]);

                        if (result.rows.length > 0) {
                            return (result.rows[0] as any).uid as UUID;
                        }

                        const insertResult = await tx.query(`
                            INSERT INTO words (uid, original_language_uid, original, original_normalized, created_at)
                            VALUES ($1, $2, $3, $4, $5)
                            RETURNING uid
                        `, [
                            generateUID(),
                            sourceLanguageUid,
                            word.grammar.originalInitialForm,
                            word.grammar.originalInitialForm.toLowerCase(),
                            Date.now()
                        ]);

                        return (insertResult.rows[0] as any).uid as UUID;
                    })();

                    // Get or create word translation
                    const wordTranslationUid = await (async (): Promise<UUID> => {
                        const result = await tx.query(`
                            SELECT uid FROM word_translations 
                            WHERE original_word_uid = $1 
                            AND translation_normalized = $2 
                            AND language_uid = $3
                        `, [
                            originalWordUid,
                            word.grammar.targetInitialForm.toLowerCase(),
                            targetLanguageUid
                        ]);

                        if (result.rows.length > 0) {
                            return (result.rows[0] as any).uid as UUID;
                        }

                        const insertResult = await tx.query(`
                            INSERT INTO word_translations (
                                uid, language_uid, original_word_uid, translation, 
                                translation_normalized, created_at
                            )
                            VALUES ($1, $2, $3, $4, $5, $6)
                            RETURNING uid
                        `, [
                            generateUID(),
                            targetLanguageUid,
                            originalWordUid,
                            word.grammar.targetInitialForm,
                            word.grammar.targetInitialForm.toLowerCase(),
                            Date.now()
                        ]);

                        return (insertResult.rows[0] as any).uid as UUID;
                    })();

                    // Create sentence word translation
                    await tx.query(`
                        INSERT INTO sentence_word_translations (
                            uid, sentence_uid, "order", original, is_punctuation,
                            is_standalone_punctuation, is_opening_parenthesis, is_closing_parenthesis,
                            word_translation_uid, word_translation_in_context, grammar_context, note, created_at
                        )
                        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
                    `, [
                        generateUID(),
                        sentenceTranslationUid,
                        wordOrder,
                        word.original,
                        word.isPunctuation ?? false,
                        word.isStandalonePunctuation ?? false,
                        word.isOpeningParenthesis ?? false,
                        word.isClosingParenthesis ?? false,
                        wordTranslationUid,
                        word.translations,
                        JSON.stringify(word.grammar),
                        word.note,
                        Date.now()
                    ]);
                }

                wordOrder += 1;
            }
            sentenceOrder += 1;
        }
    });
    
    const totalTime = performance.now() - startTime;
    console.log(`SQL: addTranslation total time: ${totalTime.toFixed(2)}ms for paragraphUid ${paragraphUid}`);
}

export default { getLibrary, getUntranslatedParagraphs, getParagraph, addTranslation, resetDatabase, getPgInstance };