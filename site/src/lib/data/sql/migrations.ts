import { type Database } from "./sqlWorker";

const migrations = [
    {id: 1, callback: initialMigration},
    {id: 2, callback: moreIndexes},
]

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
        const tableName = "language";
        db.exec(`
            CREATE TABLE ${tableName} (
                ${entityCommon}
                code TEXT NOT NULL UNIQUE
            );
            CREATE INDEX IF NOT EXISTS idx_${tableName}_code ON ${tableName}(code);
            `);
        createCommonIndexes(db, tableName);
    }
    {
        const tableName = "word";
        db.exec(`
            CREATE TABLE ${tableName} (
                ${entityCommon}
                originalLanguageUid BLOB NOT NULL,
                original TEXT NOT NULL,
                FOREIGN KEY (originalLanguageUid) REFERENCES language(uid) ON DELETE CASCADE
            );
            -- Individual indexes to support common filtered queries
            CREATE INDEX IF NOT EXISTS idx_${tableName}_originalLanguageUid ON ${tableName}(originalLanguageUid);
            CREATE INDEX IF NOT EXISTS idx_${tableName}_lower_original ON ${tableName}(original);
            -- Expression index for case-insensitive lookups
            CREATE INDEX IF NOT EXISTS idx_${tableName}_lower_original ON ${tableName}(lower(original));
            -- Optional composite uniqueness (language + original)
            CREATE UNIQUE INDEX IF NOT EXISTS idx_${tableName}_uniq_lang_original ON ${tableName}(originalLanguageUid, original);
            CREATE UNIQUE INDEX IF NOT EXISTS idx_${tableName}_uniq_lang_original ON ${tableName}(originalLanguageUid, lower(original));
        `);
        createCommonIndexes(db, tableName);
    }
    {
        const tableName = "word_translation";
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
            CREATE INDEX IF NOT EXISTS idx_${tableName}_lower_translation ON ${tableName}(translation);
            -- For fast lookup of translations per target language when iterating original words
            CREATE INDEX IF NOT EXISTS idx_${tableName}_lang_word ON ${tableName}(translationLanguageUid, originalWordUid);
            -- Expression index for normalized translation lookups
            CREATE INDEX IF NOT EXISTS idx_${tableName}_lower_translation ON ${tableName}(lower(translation));
            -- Uniqueness: one translation (case-insensitive) per (target language, original word)
            CREATE UNIQUE INDEX IF NOT EXISTS idx_${tableName}_uniq_lang_word_lower_translation ON ${tableName}(translationLanguageUid, originalWordUid, lower(translation));
            CREATE UNIQUE INDEX IF NOT EXISTS idx_${tableName}_uniq_lang_word_lower_translation ON ${tableName}(translationLanguageUid, originalWordUid, translation);
        `);
        createCommonIndexes(db, tableName);
    }
}

function moreIndexes(db: Database) {
    db.exec(`
        CREATE UNIQUE INDEX IF NOT EXISTS idx_language_code_nocase ON language(code);
        CREATE UNIQUE INDEX IF NOT EXISTS idx_language_code_nocase ON language(lower(code));
    `);
}

export function applyMigrations(db: Database) {
    initializeMigrations(db);
    for (const migration of migrations) {
        applyMigration(db, migration.id, migration.callback);
    }
}