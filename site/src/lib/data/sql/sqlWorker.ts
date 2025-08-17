import sqlite3InitModule, { type Database, type Sqlite3Static } from '@sqlite.org/sqlite-wasm';
import { applyMigrations } from './migrations';
import { DictionaryBackend } from './dictionary';

const log = (message: string, ...args: any[]) => {
  console.log(`[SQL Worker] ${message}`, ...args);
};
const error = (message: string, ...args: any[]) => {
  console.error(`[SQL Worker] ${message}`, ...args);
};

log("Hi");

function start(sqlite3: Sqlite3Static) {
  log('Running SQLite3 version', sqlite3.version.libVersion);
  const db =
    'opfs' in sqlite3
      ? new sqlite3.oo1.OpfsDb('/mydb.sqlite3')
      : new sqlite3.oo1.DB('/mydb.sqlite3', 'ct');
  log(
    'opfs' in sqlite3
      ? `OPFS is available, created persisted database at ${db.filename}`
      : `OPFS is not available, created transient database ${db.filename}`,
  );
  return db;
};

function initialize(db: Database) {
  applyMigrations(db);

  const dbBackend = new DictionaryBackend(db);

  self.addEventListener('message', (event: MessageEvent) => {
    const { data } = event;
    if (!data || typeof data !== 'object') return;
    if (data.type === 'init-dictionary-port' && event.ports && event.ports[0]) {
      dbBackend.attachPort(event.ports[0]);
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

initializeSQLite();

export { type Database };