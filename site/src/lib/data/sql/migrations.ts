import type { PGliteInterface, Transaction } from "@electric-sql/pglite";

async function initialState(db: Transaction) {
    const version = 1;

    console.log(`Applying migration version ${1}`);

    // Check if migration is already applied
    const result = await db.query(`
        SELECT 1 FROM information_schema.tables 
        WHERE table_name = 'migrations'
    `);
    
    if (result.rows.length > 0) {
        const migrationCheck = await db.query(`
            SELECT 1 FROM migrations WHERE version = $1
        `, [version]);
        
        if (migrationCheck.rows.length > 0) {
            console.log(`Migration version ${version} already applied, skipping...`);
            return;
        }
    }

    // Create version table
    await db.exec(`
        CREATE TABLE IF NOT EXISTS migrations (
            version BIGINT PRIMARY KEY,
            applied_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        );
    `);

    // Insert initial migration record
    await db.query(`
        INSERT INTO migrations (version, applied_at) 
        VALUES ($1, NOW()) 
        ON CONFLICT (version) DO NOTHING;
    `, [version]);

    // Create books table
    await db.exec(`
        CREATE TABLE IF NOT EXISTS books (
            uid UUID PRIMARY KEY,
            title TEXT NOT NULL,
            path TEXT[], -- Array of strings for folder hierarchy
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        );
        CREATE INDEX IF NOT EXISTS idx_books_title ON books(title);
        CREATE INDEX IF NOT EXISTS idx_books_created_at ON books(created_at);
    `);

    // Create book_chapters table
    await db.exec(`
        CREATE TABLE IF NOT EXISTS book_chapters (
            uid UUID PRIMARY KEY,
            book_uid UUID NOT NULL REFERENCES books(uid) ON DELETE CASCADE,
            "order" INTEGER NOT NULL,
            title TEXT,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        );
        CREATE INDEX IF NOT EXISTS idx_book_chapters_book_uid ON book_chapters(book_uid);
        CREATE INDEX IF NOT EXISTS idx_book_chapters_order ON book_chapters("order");
        CREATE INDEX IF NOT EXISTS idx_book_chapters_created_at ON book_chapters(created_at);
    `);

    // Create paragraphs table
    await db.exec(`
        CREATE TABLE IF NOT EXISTS paragraphs (
            uid UUID PRIMARY KEY,
            chapter_uid UUID NOT NULL REFERENCES book_chapters(uid) ON DELETE CASCADE,
            "order" INTEGER NOT NULL,
            original_text TEXT NOT NULL,
            original_html TEXT,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        );
        CREATE INDEX IF NOT EXISTS idx_paragraphs_chapter_uid ON paragraphs(chapter_uid);
        CREATE INDEX IF NOT EXISTS idx_paragraphs_order ON paragraphs("order");
        CREATE INDEX IF NOT EXISTS idx_paragraphs_created_at ON paragraphs(created_at);
    `);

    // Create languages table
    await db.exec(`
        CREATE TABLE IF NOT EXISTS languages (
            uid UUID PRIMARY KEY,
            name TEXT NOT NULL,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        );
        CREATE INDEX IF NOT EXISTS idx_languages_name ON languages(name);
        CREATE INDEX IF NOT EXISTS idx_languages_created_at ON languages(created_at);
    `);

    // Create paragraph_translations table
    await db.exec(`
        CREATE TABLE IF NOT EXISTS paragraph_translations (
            uid UUID PRIMARY KEY,
            paragraph_uid UUID NOT NULL REFERENCES paragraphs(uid) ON DELETE CASCADE,
            language_uid UUID NOT NULL REFERENCES languages(uid) ON DELETE CASCADE,
            translating_model TEXT NOT NULL,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        );
        CREATE INDEX IF NOT EXISTS idx_paragraph_translations_paragraph_uid ON paragraph_translations(paragraph_uid);
        CREATE INDEX IF NOT EXISTS idx_paragraph_translations_language_uid ON paragraph_translations(language_uid);
        CREATE INDEX IF NOT EXISTS idx_paragraph_translations_created_at ON paragraph_translations(created_at);
    `);

    // Create sentence_translations table
    await db.exec(`
        CREATE TABLE IF NOT EXISTS sentence_translations (
            uid UUID PRIMARY KEY,
            paragraph_translation_uid UUID NOT NULL REFERENCES paragraph_translations(uid) ON DELETE CASCADE,
            "order" INTEGER NOT NULL,
            full_translation TEXT NOT NULL,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        );
        CREATE INDEX IF NOT EXISTS idx_sentence_translations_paragraph_translation_uid ON sentence_translations(paragraph_translation_uid);
        CREATE INDEX IF NOT EXISTS idx_sentence_translations_order ON sentence_translations("order");
        CREATE INDEX IF NOT EXISTS idx_sentence_translations_created_at ON sentence_translations(created_at);
    `);

    // Create words table
    await db.exec(`
        CREATE TABLE IF NOT EXISTS words (
            uid UUID PRIMARY KEY,
            original_language_uid UUID NOT NULL REFERENCES languages(uid) ON DELETE CASCADE,
            original TEXT NOT NULL,
            original_normalized TEXT NOT NULL,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        );
        CREATE INDEX IF NOT EXISTS idx_words_original_language_uid ON words(original_language_uid);
        CREATE INDEX IF NOT EXISTS idx_words_original ON words(original);
        CREATE INDEX IF NOT EXISTS idx_words_original_normalized ON words(original_normalized);
        CREATE INDEX IF NOT EXISTS idx_words_created_at ON words(created_at);
    `);

    // Create word_translations table
    await db.exec(`
        CREATE TABLE IF NOT EXISTS word_translations (
            uid UUID PRIMARY KEY,
            language_uid UUID NOT NULL REFERENCES languages(uid) ON DELETE CASCADE,
            original_word_uid UUID NOT NULL REFERENCES words(uid) ON DELETE CASCADE,
            translation TEXT NOT NULL,
            translation_normalized TEXT NOT NULL,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        );
        CREATE INDEX IF NOT EXISTS idx_word_translations_language_uid ON word_translations(language_uid);
        CREATE INDEX IF NOT EXISTS idx_word_translations_original_word_uid ON word_translations(original_word_uid);
        CREATE INDEX IF NOT EXISTS idx_word_translations_translation ON word_translations(translation);
        CREATE INDEX IF NOT EXISTS idx_word_translations_translation_normalized ON word_translations(translation_normalized);
        CREATE INDEX IF NOT EXISTS idx_word_translations_created_at ON word_translations(created_at);
    `);

    // Create sentence_word_translations table
    await db.exec(`
        CREATE TABLE IF NOT EXISTS sentence_word_translations (
            uid UUID PRIMARY KEY,
            sentence_uid UUID NOT NULL REFERENCES sentence_translations(uid) ON DELETE CASCADE,
            "order" INTEGER NOT NULL,
            original TEXT NOT NULL,
            is_punctuation BOOLEAN NOT NULL DEFAULT FALSE,
            is_standalone_punctuation BOOLEAN NOT NULL DEFAULT FALSE,
            is_opening_parenthesis BOOLEAN NOT NULL DEFAULT FALSE,
            is_closing_parenthesis BOOLEAN NOT NULL DEFAULT FALSE,
            word_translation_uid UUID REFERENCES word_translations(uid) ON DELETE SET NULL,
            word_translation_in_context TEXT[], -- Array of strings
            grammar_context JSONB, -- JSON object for Grammar interface
            note TEXT,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        );
        CREATE INDEX IF NOT EXISTS idx_sentence_word_translations_sentence_uid ON sentence_word_translations(sentence_uid);
        CREATE INDEX IF NOT EXISTS idx_sentence_word_translations_order ON sentence_word_translations("order");
        CREATE INDEX IF NOT EXISTS idx_sentence_word_translations_original ON sentence_word_translations(original);
        CREATE INDEX IF NOT EXISTS idx_sentence_word_translations_word_translation_uid ON sentence_word_translations(word_translation_uid);
        CREATE INDEX IF NOT EXISTS idx_sentence_word_translations_created_at ON sentence_word_translations(created_at);
    `);

    console.log(`Migration version ${version} is applied`);
}

async function hashIndexes(db: Transaction) {
    const version = 2;

    console.log(`Applying migration version ${version}`);

    // Check if migration is already applied
    const migrationCheck = await db.query(`
        SELECT 1 FROM migrations WHERE version = $1
    `, [version]);
    
    if (migrationCheck.rows.length > 0) {
        console.log(`Migration version ${version} already applied, skipping...`);
        return;
    }

    // Convert UUID field indexes to hash indexes
    // Drop existing B-tree indexes and recreate as hash indexes
    
    await db.exec(`
        -- book_chapters table UUID indexes
        DROP INDEX IF EXISTS idx_book_chapters_book_uid;
        CREATE INDEX idx_book_chapters_book_uid ON book_chapters USING HASH (book_uid);
        
        -- paragraphs table UUID indexes
        DROP INDEX IF EXISTS idx_paragraphs_chapter_uid;
        CREATE INDEX idx_paragraphs_chapter_uid ON paragraphs USING HASH (chapter_uid);
        
        -- paragraph_translations table UUID indexes
        DROP INDEX IF EXISTS idx_paragraph_translations_paragraph_uid;
        CREATE INDEX idx_paragraph_translations_paragraph_uid ON paragraph_translations USING HASH (paragraph_uid);
        
        DROP INDEX IF EXISTS idx_paragraph_translations_language_uid;
        CREATE INDEX idx_paragraph_translations_language_uid ON paragraph_translations USING HASH (language_uid);
        
        -- sentence_translations table UUID indexes
        DROP INDEX IF EXISTS idx_sentence_translations_paragraph_translation_uid;
        CREATE INDEX idx_sentence_translations_paragraph_translation_uid ON sentence_translations USING HASH (paragraph_translation_uid);
        
        -- words table UUID indexes
        DROP INDEX IF EXISTS idx_words_original_language_uid;
        CREATE INDEX idx_words_original_language_uid ON words USING HASH (original_language_uid);
        
        -- word_translations table UUID indexes
        DROP INDEX IF EXISTS idx_word_translations_language_uid;
        CREATE INDEX idx_word_translations_language_uid ON word_translations USING HASH (language_uid);
        
        DROP INDEX IF EXISTS idx_word_translations_original_word_uid;
        CREATE INDEX idx_word_translations_original_word_uid ON word_translations USING HASH (original_word_uid);
        
        -- sentence_word_translations table UUID indexes
        DROP INDEX IF EXISTS idx_sentence_word_translations_sentence_uid;
        CREATE INDEX idx_sentence_word_translations_sentence_uid ON sentence_word_translations USING HASH (sentence_uid);
        
        DROP INDEX IF EXISTS idx_sentence_word_translations_word_translation_uid;
        CREATE INDEX idx_sentence_word_translations_word_translation_uid ON sentence_word_translations USING HASH (word_translation_uid);
    `);

    // Insert migration record
    await db.query(`
        INSERT INTO migrations (version, applied_at) 
        VALUES ($1, NOW());
    `, [version]);

    console.log(`Migration version ${version} is applied`);
}

async function primaryKeyHashIndexes(db: Transaction) {
    const version = 3;

    console.log(`Applying migration version ${version}`);

    // Check if migration is already applied
    const migrationCheck = await db.query(`
        SELECT 1 FROM migrations WHERE version = $1
    `, [version]);
    
    if (migrationCheck.rows.length > 0) {
        console.log(`Migration version ${version} already applied, skipping...`);
        return;
    }

    // Create hash indexes on primary keys for better UUID lookup performance
    // Note: Primary key constraints already provide unique B-tree indexes,
    // but hash indexes can be faster for equality lookups on UUIDs
    
    await db.exec(`
        -- Create hash indexes on primary keys
        CREATE INDEX IF NOT EXISTS idx_books_uid_hash ON books USING HASH (uid);
        CREATE INDEX IF NOT EXISTS idx_book_chapters_uid_hash ON book_chapters USING HASH (uid);
        CREATE INDEX IF NOT EXISTS idx_paragraphs_uid_hash ON paragraphs USING HASH (uid);
        CREATE INDEX IF NOT EXISTS idx_languages_uid_hash ON languages USING HASH (uid);
        CREATE INDEX IF NOT EXISTS idx_paragraph_translations_uid_hash ON paragraph_translations USING HASH (uid);
        CREATE INDEX IF NOT EXISTS idx_sentence_translations_uid_hash ON sentence_translations USING HASH (uid);
        CREATE INDEX IF NOT EXISTS idx_words_uid_hash ON words USING HASH (uid);
        CREATE INDEX IF NOT EXISTS idx_word_translations_uid_hash ON word_translations USING HASH (uid);
        CREATE INDEX IF NOT EXISTS idx_sentence_word_translations_uid_hash ON sentence_word_translations USING HASH (uid);
    `);

    // Insert migration record
    await db.query(`
        INSERT INTO migrations (version, applied_at) 
        VALUES ($1, NOW());
    `, [version]);

    console.log(`Migration version ${version} is applied`);
}

export async function applyMigrations(db: PGliteInterface) {
    await db.transaction(async tx => 
        {
            await initialState(tx);
            await hashIndexes(tx);
            await primaryKeyHashIndexes(tx);
        }
    );
}