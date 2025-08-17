import type { Database } from "@sqlite.org/sqlite-wasm";
import { generateUID, type UUID } from "../v2/db";

// -----------------------
// Messaging Types
// -----------------------
export type AddTranslationMessage = {
    originalWord: string,
    originalLanguageCode: string,
    targetWord: string,
    targetLanguageCode: string,
};

type DictionaryRequest = {
    id: number;
    action: 'addTranslation';
    payload: AddTranslationMessage;
};

type DictionarySuccessResponse = {
    id: number;
    result: UUID;
};

type DictionaryErrorResponse = {
    id: number;
    error: string;
};

type DictionaryResponse = DictionarySuccessResponse | DictionaryErrorResponse;

// -----------------------
// Main-thread Wrapper (communicates with worker backend)
// -----------------------
export class DictionaryWrapper {
    private port?: MessagePort;
    private requestId = 0;
    private pending = new Map<number, { resolve: (v: UUID) => void; reject: (e: any) => void }>();

    attachPort(port: MessagePort) {
        if (this.port) return; // attach only once
        this.port = port;
        this.port.onmessage = (event: MessageEvent<DictionaryResponse>) => {
            const data = event.data;
            if (!data || typeof data !== 'object' || typeof data.id !== 'number') return;
            const handler = this.pending.get(data.id);
            if (!handler) return;
            this.pending.delete(data.id);
            if ('error' in data) {
                handler.reject(new Error(data.error));
            } else {
                handler.resolve(data.result);
            }
        };
    }

    private ensurePort(): MessagePort {
        if (!this.port) throw new Error('DictionaryWrapper: port not attached');
        return this.port;
    }

    addTranslation(message: AddTranslationMessage): Promise<UUID> {
        const port = this.ensurePort();
        const id = ++this.requestId;
        const req: DictionaryRequest = { id, action: 'addTranslation', payload: message };
        return new Promise<UUID>((resolve, reject) => {
            this.pending.set(id, { resolve, reject });
            try {
                port.postMessage(req);
            } catch (err) {
                this.pending.delete(id);
                reject(err);
            }
        });
    }
}

// Singleton instance exported for convenience (must be initialized via initDictionaryMessaging)
export const dictionary = new DictionaryWrapper();

// Initialize messaging by creating a MessageChannel and passing one port to the worker.
// Returns the wrapper instance ready for use.
export function initDictionaryMessaging(worker: Worker) {
    const channel = new MessageChannel();
    dictionary.attachPort(channel.port1);
    worker.postMessage({ type: 'init-dictionary-port' }, [channel.port2]);
    return dictionary;
}

export class DictionaryBackend {
    constructor(private db: Database) { }

    // -----------------------
    // In-memory caches
    // -----------------------
    // Key: lower(languageCode) -> languageUid
    private languageCache = new Map<string, UUID>();
    // Key: languageUid|lower(word) -> wordUid
    private wordCache = new Map<string, UUID>();
    // Key: translationLanguageUid|originalWordUid|lower(translation) -> translationUid
    private translationCache = new Map<string, UUID>();

    private getOrInsertLanguage(db: Database, code: string, proposedUid: UUID, now: number): UUID {
        const norm = code.trim().toLowerCase();
        const cached = this.languageCache.get(norm);
        if (cached) return cached;
        const existing = db.selectValue(
            "SELECT uid FROM language WHERE lower(code)=lower(?) LIMIT 1",
            [code]
        ) as UUID | undefined;
        const uid = existing ?? (() => {
            db.exec(
                "INSERT INTO language(uid, code, createdAt, updatedAt) VALUES(?1, ?2, ?3, ?3)",
                { bind: [proposedUid, code, now] }
            );
            return proposedUid;
        })();
        this.languageCache.set(norm, uid);
        return uid;
    }

    private getOrInsertWord(db: Database, languageUid: UUID, word: string, proposedUid: UUID, now: number): UUID {
        const norm = word.trim().toLowerCase();
        const key = `${languageUid}|${norm}`;
        const cached = this.wordCache.get(key);
        if (cached) return cached;
        const existing = db.selectValue(
            "SELECT uid FROM word WHERE originalLanguageUid=?1 AND lower(original)=lower(?2) LIMIT 1",
            [languageUid, word]
        ) as UUID | undefined;
        const uid = existing ?? (() => {
            db.exec(
                "INSERT INTO word(uid, originalLanguageUid, original, createdAt, updatedAt) VALUES(?1, ?2, ?3, ?4, ?4)",
                { bind: [proposedUid, languageUid, word, now] }
            );
            return proposedUid;
        })();
        this.wordCache.set(key, uid);
        return uid;
    }

    private getOrInsertTranslation(
        db: Database,
        translationLanguageUid: UUID,
        originalWordUid: UUID,
        translation: string,
        proposedUid: UUID,
        now: number
    ): UUID {
        const norm = translation.trim().toLowerCase();
        const key = `${translationLanguageUid}|${originalWordUid}|${norm}`;
        const cached = this.translationCache.get(key);
        if (cached) return cached;
        const existing = db.selectValue(
            `SELECT uid FROM word_translation
                     WHERE translationLanguageUid=?1
                       AND originalWordUid=?2
                       AND lower(translation)=lower(?3)
                     LIMIT 1`,
            [translationLanguageUid, originalWordUid, translation]
        ) as UUID | undefined;
        const uid = existing ?? (() => {
            db.exec(
                `INSERT INTO word_translation
                        (uid, translationLanguageUid, originalWordUid, translation, createdAt, updatedAt)
                     VALUES(?1, ?2, ?3, ?4, ?5, ?5)`,
                { bind: [proposedUid, translationLanguageUid, originalWordUid, translation, now] }
            );
            return proposedUid;
        })();
        this.translationCache.set(key, uid);
        return uid;
    }

    addTranslation(message: AddTranslationMessage): UUID {
        const now = Date.now();
        const origLangUid = generateUID();
        const targetLangUid = generateUID();
        const originalWordUid = generateUID();
        const translationUid = generateUID();

        let resultUid: UUID | undefined;

        this.db.transaction(db => {
            const origLang = this.getOrInsertLanguage(db, message.originalLanguageCode, origLangUid, now);
            const targetLang = this.getOrInsertLanguage(db, message.targetLanguageCode, targetLangUid, now);
            const originalWord = this.getOrInsertWord(db, origLang, message.originalWord, originalWordUid, now);
            resultUid = this.getOrInsertTranslation(db, targetLang, originalWord, message.targetWord, translationUid, now);
        });

        if (!resultUid) throw new Error("Failed to insert or retrieve translation UID");
        return resultUid;
    }

    // Attach a MessagePort for handling incoming dictionary requests inside the worker context
    attachPort(port: MessagePort) {
        port.onmessage = (ev: MessageEvent) => {
            const msg = ev.data;
            if (!msg || typeof msg !== 'object') return;
            const { id, action, payload } = msg as { id: number; action: string; payload: any };
            if (typeof id !== 'number') return;
            if (action === 'addTranslation') {
                try {
                    const result = this.addTranslation(payload as AddTranslationMessage);
                    port.postMessage({ id, result });
                } catch (e: any) {
                    port.postMessage({ id, error: e?.message || 'Unknown error' });
                }
            }
        };
    }
}