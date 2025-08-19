import sqlite3InitModule, { type Database, type Sqlite3Static } from '@sqlite.org/sqlite-wasm';
import { applyMigrations } from './migrations';
import { DictionaryBackend } from './dictionary';
import { BookBackend } from './book';
import type { UUID } from '../v2/db';

// NOTE: Keep this union in sync with all tables created in migrations.ts
export type TableName =
  | "migrations"
  | "language"
  | "word"
  | "word_translation"
  | "book"
  | "book_chapter"
  | "book_chapter_paragraph"
  | "book_chapter_paragraph_translation"
  | "book_paragraph_translation_sentence"
  | "book_paragraph_translation_sentence_word";

export type DbUpdateMessage = {
  table: TableName,
  uid: UUID,
  action: 'insert' | 'update' | 'delete',
};

export const dbUpdatesChannelName = "db_updates_channel";

const log = (message: string, ...args: any[]) => {
  console.log(`[SQL Worker] ${message}`, ...args);
};
const error = (message: string, ...args: any[]) => {
  console.error(`[SQL Worker] ${message}`, ...args);
};

function start(sqlite3: Sqlite3Static) {
  log('Running SQLite3 version', sqlite3.version.libVersion);
  const db =
    'opfs' in sqlite3
      ? new sqlite3.oo1.OpfsDb('/db3.sqlite3')
      : new sqlite3.oo1.DB('/db3.sqlite3', 'ct');
  log(
    'opfs' in sqlite3
      ? `OPFS is available, created persisted database at ${db.filename}`
      : `OPFS is not available, created transient database ${db.filename}`,
  );
  return db;
};

function initialize(db: Database) {
  applyMigrations(db);

  const dictionaryBackend = new DictionaryBackend(db);
  const bookBackend = new BookBackend(db);

  self.addEventListener('message', (event: MessageEvent) => {
    const { data } = event;
    if (!data || typeof data !== 'object') return;
    if (data.type === 'init-dictionary-port' && event.ports && event.ports[0]) {
      dictionaryBackend.attachPort(event.ports[0]);
    } else if (data.type === 'init-book-port' && event.ports && event.ports[0]) {
      bookBackend.attachPort(event.ports[0]);
    }
  });
}

async function initializeSQLite() {
  try {
    log('Loading and initializing SQLite3 module...');
    const sqlite3 = await sqlite3InitModule({ print: log, printErr: error });
    const db = start(sqlite3);
    initialize(db);
    self.postMessage({ type: 'ready' });
    log('Done initializing.');
  } catch (err: any) {
    log(err);
    error('Initialization error:', err?.name, err?.message);
  }
};

if (typeof window === 'undefined' && typeof document === 'undefined') {
  initializeSQLite();
}

export { type Database };