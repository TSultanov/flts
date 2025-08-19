import { type Database, type TableName } from "./sqlWorker";

const migrations = [
    { id: 1, callback: initialMigration },
    { id: 2, callback: createBookTables },
];

const entityCommon = `
    uid BLOB PRIMARY KEY,
    createdAt DATETIME,
    updatedAt DATETIME,
`;

function createCommonIndexes(db: Database, tableName: string) {
    db.exec(`
        CREATE INDEX IF NOT EXISTS idx_${tableName}_createdAt ON ${tableName}(createdAt);
        CREATE INDEX IF NOT EXISTS idx_${tableName}_updatedAt ON ${tableName}(updatedAt);
        `);
}

function initializeMigrations(db: Database) {
    db.transaction(db => {
        db.exec(`CREATE TABLE IF NOT EXISTS migrations (
                id INTEGER PRIMARY KEY,
                appliedAt DATETIME
            );
            CREATE INDEX IF NOT EXISTS idx_migrations_appliedAt ON migrations(appliedAt);`);
    });
}

function applyMigration(db: Database, id: number, migration: (db: Database) => void) {
    db.transaction(db => {
        // Check if migration with this id is already applied, if yes - skip
        const existingId = db.selectValue("SELECT id FROM migrations WHERE id = ?", [id]);
        
        if (existingId === id) {
            return;
        }

        // Otherwise apply migration
        migration(db);
        db.exec({
            sql: "INSERT INTO migrations (id, appliedAt) VALUES (?, ?)",
            bind: [id, Date.now()]
        });
    })
}

function initialMigration(db: Database) {
    // Ensure foreign key enforcement
    db.exec(`PRAGMA foreign_keys = ON;`);
    {
        const tableName: TableName = "language";
        db.exec(`
            CREATE TABLE ${tableName} (
                ${entityCommon}
                code TEXT NOT NULL UNIQUE
            );
            CREATE INDEX IF NOT EXISTS idx_language_lower_code ON language(lower(code));
            -- Additional NOCASE unique index to support ON CONFLICT(code) with case-insensitive semantics
            CREATE UNIQUE INDEX IF NOT EXISTS ux_language_code_nocase ON language(code COLLATE NOCASE);
            `);
        createCommonIndexes(db, tableName);
    }
    {
        const tableName: TableName = "word";
        db.exec(`
            CREATE TABLE ${tableName} (
                ${entityCommon}
                originalLanguageUid BLOB NOT NULL,
                original TEXT NOT NULL,
                FOREIGN KEY (originalLanguageUid) REFERENCES language(uid) ON DELETE CASCADE
            );
            -- Individual indexes to support common filtered queries
            CREATE INDEX IF NOT EXISTS idx_${tableName}_originalLanguageUid ON ${tableName}(originalLanguageUid);
            CREATE INDEX IF NOT EXISTS idx_${tableName}_original ON ${tableName}(original);
            -- Expression index for case-insensitive lookups
            CREATE INDEX IF NOT EXISTS idx_${tableName}_lower_original ON ${tableName}(lower(original));
            -- Case-insensitive uniqueness (language + original) using lower(original)
            CREATE UNIQUE INDEX IF NOT EXISTS idx_${tableName}_uniq_lang_lower_original ON ${tableName}(originalLanguageUid, lower(original));
            -- Additional NOCASE unique index to allow ON CONFLICT(originalLanguageUid, original)
            CREATE UNIQUE INDEX IF NOT EXISTS ux_word_lang_original_nocase ON ${tableName}(originalLanguageUid, original COLLATE NOCASE);
        `);
        createCommonIndexes(db, tableName);
    }
    {
        const tableName: TableName = "word_translation";
        db.exec(`
            CREATE TABLE ${tableName} (
                ${entityCommon}
                translationLanguageUid BLOB NOT NULL,
                originalWordUid BLOB NOT NULL,
                translation TEXT NOT NULL,
                FOREIGN KEY (translationLanguageUid) REFERENCES language(uid) ON DELETE CASCADE,
                FOREIGN KEY (originalWordUid) REFERENCES word(uid) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS idx_${tableName}_translationLanguageUid ON ${tableName}(translationLanguageUid);
            CREATE INDEX IF NOT EXISTS idx_${tableName}_originalWordUid ON ${tableName}(originalWordUid);
            -- Separate plain and lower() indexes with distinct names
            CREATE INDEX IF NOT EXISTS idx_${tableName}_translation ON ${tableName}(translation);
            CREATE INDEX IF NOT EXISTS idx_${tableName}_lower_translation ON ${tableName}(lower(translation));
            -- For fast lookup of translations per target language when iterating original words
            CREATE INDEX IF NOT EXISTS idx_${tableName}_lang_word ON ${tableName}(translationLanguageUid, originalWordUid);
            -- Case-insensitive uniqueness: one translation per (target language, original word)
            CREATE UNIQUE INDEX IF NOT EXISTS idx_${tableName}_uniq_lang_word_lower_translation ON ${tableName}(translationLanguageUid, originalWordUid, lower(translation));
            -- Additional NOCASE unique index to allow ON CONFLICT(translationLanguageUid, originalWordUid, translation)
            CREATE UNIQUE INDEX IF NOT EXISTS ux_translation_combo_nocase ON ${tableName}(translationLanguageUid, originalWordUid, translation COLLATE NOCASE);
        `);
        createCommonIndexes(db, tableName);
    }
}

function createBookTables(db: Database) {
    // BOOK
    {
        const tableName: TableName = "book";
        db.exec(`
            CREATE TABLE IF NOT EXISTS ${tableName} (
                ${entityCommon}
                path TEXT NOT NULL, -- JSON encoded string[] representing folder path
                title TEXT NOT NULL,
                chapterCount INTEGER NOT NULL,
                paragraphCount INTEGER NOT NULL,
                translatedParagraphsCount INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS idx_${tableName}_title ON ${tableName}(title);
            CREATE INDEX IF NOT EXISTS idx_${tableName}_lower_title ON ${tableName}(lower(title));
        `);
        createCommonIndexes(db, tableName);
    }

    // CHAPTER
    {
        const tableName: TableName = "book_chapter";
        db.exec(`
            CREATE TABLE IF NOT EXISTS ${tableName} (
                ${entityCommon}
                bookUid BLOB NOT NULL,
                chapterIndex INTEGER NOT NULL,
                title TEXT,
                FOREIGN KEY (bookUid) REFERENCES book(uid) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS idx_${tableName}_bookUid ON ${tableName}(bookUid);
            CREATE INDEX IF NOT EXISTS idx_${tableName}_chapterIndex ON ${tableName}(chapterIndex);
            CREATE UNIQUE INDEX IF NOT EXISTS ux_${tableName}_book_chapterIndex ON ${tableName}(bookUid, chapterIndex);
        `);
        createCommonIndexes(db, tableName);
    }

    // PARAGRAPH
    {
        const tableName: TableName = "book_chapter_paragraph";
        db.exec(`
            CREATE TABLE IF NOT EXISTS ${tableName} (
                ${entityCommon}
                chapterUid BLOB NOT NULL,
                paragraphIndex INTEGER NOT NULL,
                originalText TEXT NOT NULL,
                originalHtml TEXT,
                FOREIGN KEY (chapterUid) REFERENCES book_chapter(uid) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS idx_${tableName}_chapterUid ON ${tableName}(chapterUid);
            CREATE INDEX IF NOT EXISTS idx_${tableName}_paragraphIndex ON ${tableName}(paragraphIndex);
            CREATE UNIQUE INDEX IF NOT EXISTS ux_${tableName}_chapter_paragraphIndex ON ${tableName}(chapterUid, paragraphIndex);
        `);
        createCommonIndexes(db, tableName);
    }

    // PARAGRAPH TRANSLATION (paragraph-level)
    {
        const tableName: TableName = "book_chapter_paragraph_translation";
        db.exec(`
            CREATE TABLE IF NOT EXISTS ${tableName} (
                ${entityCommon}
                chapterParagraphUid BLOB NOT NULL,
                languageUid BLOB NOT NULL,
                translatingModel TEXT NOT NULL,
                translationJson TEXT, -- JSON blob with translation data
                FOREIGN KEY (chapterParagraphUid) REFERENCES book_chapter_paragraph(uid) ON DELETE CASCADE,
                FOREIGN KEY (languageUid) REFERENCES language(uid) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS idx_${tableName}_chapterParagraphUid ON ${tableName}(chapterParagraphUid);
            CREATE INDEX IF NOT EXISTS idx_${tableName}_languageUid ON ${tableName}(languageUid);
            CREATE UNIQUE INDEX IF NOT EXISTS ux_${tableName}_paragraph_language ON ${tableName}(chapterParagraphUid, languageUid);
        `);
        createCommonIndexes(db, tableName);
    }
    {
        const tableName: TableName = "book_paragraph_translation_sentence";
        db.exec(`
            CREATE TABLE IF NOT EXISTS ${tableName} (
                ${entityCommon}
                paragraphTranslationUid BLOB NOT NULL,
                sentenceIndex INTEGER NOT NULL,
                fullTranslation TEXT NOT NULL,
                FOREIGN KEY (paragraphTranslationUid) REFERENCES book_chapter_paragraph_translation(uid) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS idx_${tableName}_paragraphTranslationUid ON ${tableName}(paragraphTranslationUid);
            CREATE INDEX IF NOT EXISTS idx_${tableName}_sentenceIndex ON ${tableName}(sentenceIndex);
            CREATE UNIQUE INDEX IF NOT EXISTS ux_${tableName}_translation_sentenceIndex ON ${tableName}(paragraphTranslationUid, sentenceIndex);
        `);
        createCommonIndexes(db, tableName);
    }
    {
        const tableName: TableName = "book_paragraph_translation_sentence_word";
        db.exec(`
            CREATE TABLE IF NOT EXISTS ${tableName} (
                ${entityCommon}
                sentenceUid BLOB NOT NULL,
                wordIndex INTEGER NOT NULL,
                original TEXT NOT NULL,
                isPunctuation INTEGER NOT NULL,
                isStandalonePunctuation INTEGER,
                isOpeningParenthesis INTEGER,
                isClosingParenthesis INTEGER,
                wordTranslationUid BLOB,
                wordTranslationInContext TEXT,
                grammarContext TEXT, -- stored as JSON blob (not normalized)
                note TEXT,
                FOREIGN KEY (sentenceUid) REFERENCES book_paragraph_translation_sentence(uid) ON DELETE CASCADE,
                FOREIGN KEY (wordTranslationUid) REFERENCES word_translation(uid) ON DELETE SET NULL
            );
            CREATE INDEX IF NOT EXISTS idx_${tableName}_sentenceUid ON ${tableName}(sentenceUid);
            CREATE INDEX IF NOT EXISTS idx_${tableName}_wordIndex ON ${tableName}(wordIndex);
            CREATE INDEX IF NOT EXISTS idx_${tableName}_wordTranslationUid ON ${tableName}(wordTranslationUid);
            CREATE UNIQUE INDEX IF NOT EXISTS ux_${tableName}_sentence_wordIndex ON ${tableName}(sentenceUid, wordIndex);
        `);
        createCommonIndexes(db, tableName);
    }
}

export function applyMigrations(db: Database) {
    initializeMigrations(db);
    for (const migration of migrations) {
        applyMigration(db, migration.id, migration.callback);
    }
}