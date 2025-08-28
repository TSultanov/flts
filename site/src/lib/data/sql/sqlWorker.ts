import sqlite3InitModule, { type Database, type Sqlite3Static } from '@sqlite.org/sqlite-wasm';
import { applyMigrations } from './migrations';
import { DictionaryBackend } from './dictionary';
import { BookBackend } from './book';
import { v4 as uuidv4 } from "uuid";
import { DB_FILE_NAME, DB_FILE_PATH } from './utils';

export type UUID = string & { readonly __brand: "UUID" };

interface StrictBroadcastChannelEventMap<T> {
  "message": MessageEvent<T>;
  "messageerror": MessageEvent<T>;
}

export interface StrictBroadcastChannel<T> extends EventTarget {
  readonly name: string;
  onmessage: ((this: BroadcastChannel, ev: MessageEvent<T>) => any) | null;
  onmessageerror: ((this: BroadcastChannel, ev: MessageEvent<T>) => any) | null;
  close(): void;
  postMessage(message: T): void;
  addEventListener<K extends keyof StrictBroadcastChannelEventMap<T>>(type: K, listener: (this: BroadcastChannel, ev: StrictBroadcastChannelEventMap<T>[K]) => any, options?: boolean | AddEventListenerOptions): void;
  addEventListener(type: string, listener: EventListenerOrEventListenerObject, options?: boolean | AddEventListenerOptions): void;
  removeEventListener<K extends keyof StrictBroadcastChannelEventMap<T>>(type: K, listener: (this: BroadcastChannel, ev: StrictBroadcastChannelEventMap<T>[K]) => any, options?: boolean | EventListenerOptions): void;
  removeEventListener(type: string, listener: EventListenerOrEventListenerObject, options?: boolean | EventListenerOptions): void;
}

function isValidUUID(value: string): value is UUID {
    const uuidRegex = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
    return uuidRegex.test(value);
}

export function createUUID(value: string): UUID {
    if (!isValidUUID(value)) {
        throw new Error(`Invalid UUID format: ${value}`);
    }
    return value as UUID;
}

export function generateUID(): UUID {
    return createUUID(uuidv4());
}

export type Entity = {
    uid: UUID,
    createdAt: number,
    updatedAt: number,
}

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

const log = (message: string, ...args: any[]) => {
  console.log(`[SQL Worker] ${message}`, ...args);
};
const error = (message: string, ...args: any[]) => {
  console.error(`[SQL Worker] ${message}`, ...args);
};

async function prepareOpfsDatabaseFile(sqlite3: Sqlite3Static) {
  // If an imported DB is staged (db3.sqlite3.import), move it into place before opening
  try {
    // Access OPFS root via sqlite3 helper (when available) or browser API
    if (!('opfs' in sqlite3)) return; // nothing to do if OPFS is not used
    const storage: any = (self as any).navigator?.storage;
    if (!storage || typeof storage.getDirectory !== 'function') return;
    const root = await storage.getDirectory();
  const dbName = DB_FILE_NAME;
  const tempName = `${dbName}.import`;

    // Check temp file existence by trying to get a handle; if not present, exit
    let tempHandle: FileSystemFileHandle | undefined;
    try {
      tempHandle = await (root as FileSystemDirectoryHandle).getFileHandle(tempName, { create: false });
    } catch {
      return; // no staged import
    }
    if (!tempHandle) return;

    // Write temp contents into the target file (create if missing)
    const targetHandle = await (root as FileSystemDirectoryHandle).getFileHandle(dbName, { create: true });
    const tempFile = await tempHandle.getFile();
    const buf = await tempFile.arrayBuffer();
    const writable = await targetHandle.createWritable();
    try {
      await writable.truncate(0);
      await writable.write(buf);
    } finally {
      await writable.close();
    }

    // Remove the temp file after successful write
    try {
      await (root as any).removeEntry(tempName);
    } catch (e) {
      // non-fatal
      log('Could not remove temp import file; ignoring', e);
    }
    log('Imported OPFS database from staged file');
  } catch (e) {
    error('Failed to prepare OPFS database file:', e);
  }
}

async function start(sqlite3: Sqlite3Static) {
  log('Running SQLite3 version', sqlite3.version.libVersion);
  if ('opfs' in sqlite3) {
    await prepareOpfsDatabaseFile(sqlite3);
  }
  const db =
    'opfs' in sqlite3
      ? new sqlite3.oo1.OpfsDb(DB_FILE_PATH)
      : new sqlite3.oo1.DB(DB_FILE_PATH, 'ct');
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
  const db = await start(sqlite3);
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