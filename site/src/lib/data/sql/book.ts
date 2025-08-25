import { readable, type Readable } from "svelte/store";
import type { ModelId } from "../translators/translator";
import { generateUID, type Entity, type UUID } from "../v2/db";
import { type StrictBroadcastChannel, type Database, type DbUpdateMessage, type TableName } from "./sqlWorker";
import type { EpubBook } from "../epubLoader";
import { decode } from 'html-entities';
import { dbUpdatesChannelName, debounce } from "./utils";

type BookData = {
    path: string[];
    readonly title: string,
    readonly chapterCount: number;
    readonly paragraphCount: number;
    readonly translatedParagraphsCount: number;
}

export type BookEntity = Entity & BookData

type BookChapter = Entity & {
    readonly title?: string,
}

export type Paragraph = Entity & {
    readonly originalText: string,
    readonly originalHtml?: string,
}

export type ParagraphTranslationShort = {
    languageCode: string
    translationJson: TranslationDenormal[]
}

type TranslationDenormal = {
    meta?: {
        sentenceTranslationUid: UUID,
        wordTranslationUid: UUID,
        offset: number,
    },
    text: string,
}

export type BookParagraphTranslation = Entity & {
    readonly languageCode: string,
    readonly translatingModel: ModelId,
    readonly translationJson?: TranslationDenormal[]
    readonly sentences?: SentenceTranslation[],
}

export type SentenceTranslation = Entity & {
    readonly paragraphTranslationUid: UUID,
    readonly translatingModel: ModelId,
    readonly fullTranslation: string,
    readonly words?: SentenceWordTranslation[],
}

export type SentenceWordTranslation = Entity & {
    readonly sentenceUid: UUID,
    readonly original: string,
    readonly isPunctuation: boolean,
    readonly isStandalonePunctuation?: boolean | null,
    readonly isOpeningParenthesis?: boolean | null,
    readonly isClosingParenthesis?: boolean | null,
    readonly wordTranslationUid?: UUID,
    readonly wordTranslationInContext?: string[],
    readonly grammarContext?: Grammar,
    readonly note?: string,
}

type Grammar = {
    originalInitialForm: string,
    targetInitialForm: string,
    partOfSpeech: string
    plurality?: string | null,
    person?: string | null,
    tense?: string | null,
    case?: string | null,
    other?: string | null,
}

export interface IParagraphView {
    get id(): UUID,
    get originalPlain(): string,
    get original(): string,
    get translation(): BookParagraphTranslation | undefined,
    get translationStore(): Readable<BookParagraphTranslation | undefined>;
}

export interface IChapterView {
    get id(): UUID,
    get title(): string | undefined,
    get paragraphs(): IParagraphView[];
}

export interface IBookMeta {
    readonly uid: UUID,
    readonly chapterCount: number;
    readonly translationRatio: number;
    readonly title: string;
    path: string[];
}

// -----------------------
// Messaging Types (Main Thread <-> Worker)
// -----------------------
export type CreateBookFromTextMessage = {
    title: string;
    text: string;
    path?: string[]; // optional path hierarchy
};

export type CreateBookFromEpubMessage = {
    epub: EpubBook;
    path?: string[];
};

// Requests which retrieve by a particular UUID must have
// this UID passed in the payload in a field which ends with 'Uid'
type BookRequestAction = {
    type: 'createBookFromText',
    payload: CreateBookFromTextBookRequestPayload,
} | {
    type: 'createBookFromEpub',
    payload: CreateBookFromEpubEpubRequestPayload,
} | {
    type: 'updateParagraphTranslation',
    payload: UpdateParagraphTranslationRequestPayload,
} | {
    type: 'updateBookPath',
    payload: UpdateBookPathRequestPayload,
} | {
    type: 'deleteBook',
    payload: { bookUid: UUID },
} | {
    type: 'listBooks',
    payload: {},
} | {
    type: 'getBookChapters',
    payload: { bookUid: UUID },
} | {
    type: 'getParagraphs',
    payload: { chapterUid: UUID },
} | {
    type: 'getParagraph',
    payload: { paragraphUid: UUID }
} | {
    type: 'getParagraphTranslation',
    payload: { paragraphUid: UUID, languageUid: UUID },
} | {
    type: 'getParagraphTranslationShort',
    payload: { paragraphUid: UUID, languageUid: UUID },
} | {
    type: 'getNotTranslatedParagraphsUids',
    payload: { bookUid: UUID },
} | {
    type: 'getWordTranslation',
    payload: { wordUid: UUID },
} | {
    type: 'getSentenceTranslation',
    payload: { sentenceUid: UUID },
};

type BookRequest = {
    id: bigint,
    action: BookRequestAction,
}

type CreateBookFromTextBookRequestPayload = CreateBookFromTextMessage;
type CreateBookFromEpubEpubRequestPayload = CreateBookFromEpubMessage;
type UpdateParagraphTranslationRequestPayload = UpdateParagraphTranslationMessage;
type UpdateBookPathRequestPayload = UpdateBookPathMessage;

type BookSuccessResponse = { id: BigInt; uids: Set<UUID>, result: any };
type BookErrorResponse = { id: BigInt; error: string };
type BookResponse = BookSuccessResponse | BookErrorResponse;

// -----------------------
// Main-thread Wrapper
// -----------------------
export class SqlBookWrapper {
    private port?: MessagePort;
    private requestId: bigint = BigInt(0);
    private pending = new Map<bigint, { resolve: (data: { uids: Set<UUID>, result: any }) => void; reject: (e: any) => void }>();
    private updatesChannel: StrictBroadcastChannel<DbUpdateMessage>;

    constructor() {
        this.updatesChannel = new BroadcastChannel(dbUpdatesChannelName);
    }

    attachPort(port: MessagePort) {
        if (this.port) return; // only attach once
        this.port = port;
        this.port.onmessage = (event: MessageEvent<BookResponse>) => {
            const data = event.data;
            if (!data || typeof data !== 'object' || typeof data.id !== 'bigint') return;
            const handler = this.pending.get(data.id);
            if (!handler) return;
            this.pending.delete(data.id);
            if ('error' in data) {
                handler.reject(new Error(data.error));
            } else {
                handler.resolve({ uids: data.uids, result: data.result });
            }
        };
    }

    private async ensurePort(): Promise<MessagePort> {
        // Wait up to 10s for port to be attached (poll every 100ms)
        const timeoutMs = 10_000;
        const intervalMs = 100;
        const start = Date.now();
        while (!this.port) {
            if (Date.now() - start >= timeoutMs) {
                throw new Error('SqlBookWrapper: port not attached (timeout)');
            }
            await new Promise(r => setTimeout(r, intervalMs));
        }
        return this.port;
    }

    private async send<TRet>(action: BookRequest['action']['type'], payload: BookRequest['action']['payload']): Promise<{ uids: Set<UUID>, result: TRet }> {
        const port = await this.ensurePort();
        const id = ++this.requestId;
        const req: BookRequest = { id, action: { type: action, payload } } as BookRequest;
        return new Promise<{ uids: Set<UUID>, result: TRet }>((resolve, reject) => {
            this.pending.set(id, { resolve, reject });
            try {
                port.postMessage(req);
            } catch (err) {
                this.pending.delete(id);
                reject(err);
            }
        });
    }

    private readable<T>(tables: TableName[], action: BookRequest['action']['type'], payload: BookRequest['action']['payload']): Readable<T | undefined>;
    private readable<T>(tables: TableName[], action: BookRequest['action']['type'], payload: BookRequest['action']['payload'], initial: T): Readable<T>;
    private readable<T>(tables: TableName[], action: BookRequest['action']['type'], payload: BookRequest['action']['payload'], initial?: T): Readable<T | undefined> | Readable<T> {
        return readable<T>(initial, (set) => {
            let uids: Set<UUID> | undefined = undefined;
            const update = debounce(() => {
                this.send<T>(action, payload).then(res => {
                    uids = res.uids;
                    set(res.result)
                });
            }, 100);
            // Need to actually filter by UUIDs of returned objects to only udpate affected queries
            // One idea is to establish a protocol where the returned obejct must list
            // all of the GUIDs of all objects returned in the projection, and on first udpate we record them
            // After that, only route updates to queries which are querying for those objects
            // Also record GUIDs of requested objects

            update();

            const listener = (ev: MessageEvent<DbUpdateMessage>) => {
                if (tables.includes(ev.data.table) &&
                    (uids === undefined || uids.has(ev.data.uid))) {
                    update();
                }
            };

            this.updatesChannel.addEventListener("message", listener);

            return () => {
                this.updatesChannel.removeEventListener("message", listener);
            }
        });
    }

    async createFromText(message: CreateBookFromTextMessage): Promise<UUID> {
        return (await this.send<UUID>('createBookFromText', message)).result;
    }

    async createFromEpub(message: CreateBookFromEpubMessage): Promise<UUID> {
        return (await this.send<UUID>('createBookFromEpub', message)).result;
    }

    async updateParagraphTranslation(message: UpdateParagraphTranslationMessage): Promise<UUID> {
        return (await this.send<UUID>('updateParagraphTranslation', message)).result;
    }

    async updateBookPath(message: UpdateBookPathMessage): Promise<UUID> {
        return (await this.send<UUID>('updateBookPath', message)).result;
    }

    async deleteBook(bookUid: UUID): Promise<UUID> {
        return (await this.send<UUID>('deleteBook', { bookUid })).result;
    }

    listBooks(): Readable<IBookMeta[]> {
        return this.readable<IBookMeta[]>(['book'], 'listBooks', {}, []);
    }

    getBookChapters(bookUid: UUID): Readable<BookChapter[]> {
        return this.readable(['book_chapter'], 'getBookChapters', { bookUid }, []);
    }

    getParagraphs(chapterUid: UUID): Readable<Paragraph[]> {
        return this.readable(['book_chapter_paragraph', 'book_chapter_paragraph_translation'], 'getParagraphs', { chapterUid }, []);
    }

    getParagraph(paragraphUid: UUID): Readable<Paragraph | undefined> {
        return this.readable(['book_chapter_paragraph', 'book_chapter_paragraph_translation'], 'getParagraph', { paragraphUid });
    }

    getParagraphTranslation(paragraphUid: UUID, languageUid: UUID): Readable<BookParagraphTranslation | undefined> {
        return this.readable(['book_chapter_paragraph', 'book_chapter_paragraph_translation', 'language'], 'getParagraphTranslation', { paragraphUid, languageUid });
    }

    getParagraphTranslationShort(paragraphUid: UUID, languageUid: UUID): Readable<ParagraphTranslationShort | undefined> {
        return this.readable(['book_chapter_paragraph', 'book_chapter_paragraph_translation', 'language'], 'getParagraphTranslationShort', { paragraphUid, languageUid });
    }

    getNotTranslatedParagraphsUids(bookUid: UUID): Readable<UUID[]> {
        return this.readable(['book_chapter_paragraph', 'book_chapter'], 'getNotTranslatedParagraphsUids', { bookUid }, []);
    }

    getWordTranslation(wordUid: UUID): Readable<SentenceWordTranslation | undefined> {
        return this.readable(['book_paragraph_translation_sentence_word'], 'getWordTranslation', { wordUid });
    }

    getSentenceTranslation(sentenceUid: UUID): Readable<SentenceTranslation | undefined> {
        return this.readable(['book_paragraph_translation_sentence'/*, 'book_chapter_paragraph_translation'*/], 'getSentenceTranslation', { sentenceUid });
    }
}

export const sqlBooks = new SqlBookWrapper();

export function initSqlBookMessaging(worker: Worker) {
    const channel = new MessageChannel();
    sqlBooks.attachPort(channel.port1);
    worker.postMessage({ type: 'init-book-port' }, [channel.port2]);
    return sqlBooks;
}

// -----------------------
// Worker-side Backend
// -----------------------
export class BookBackend {
    private updatesChannel: StrictBroadcastChannel<DbUpdateMessage>;

    constructor(private db: Database) {
        this.updatesChannel = new BroadcastChannel(dbUpdatesChannelName);
    }

    private sendUpdateMessage(message: DbUpdateMessage) {
        this.updatesChannel.postMessage(message);
    }

    private splitParagraphs(text: string): { text: string; html?: string }[] {
        return text
            .split(/\n+/)
            .map(p => p.trim())
            .filter(p => p.length > 0)
            .map(p => ({ text: p }));
    }

    attachPort(port: MessagePort) {
        const sendMessage = (message: BookResponse) => {
            port.postMessage(message);
        };

        port.onmessage = (ev: MessageEvent) => {
            const msg = ev.data;
            if (!msg || typeof msg !== 'object') return;
            const { id, action: { type, payload } } = msg as BookRequest;
            if (typeof id !== 'bigint') return;
            try {
                if (type === 'createBookFromText') {
                    const result = this.createBookFromText(payload);
                    sendMessage({ id, ...result });
                } else if (type === 'createBookFromEpub') {
                    const result = this.createBookFromEpub(payload);
                    sendMessage({ id, ...result });
                } else if (type === 'updateParagraphTranslation') {
                    const result = this.updateParagraphTranslation(payload);
                    sendMessage({ id, ...result });
                } else if (type === 'updateBookPath') {
                    const result = this.updateBookPath(payload);
                    sendMessage({ id, ...result });
                } else if (type === 'deleteBook') {
                    const result = this.deleteBook(payload.bookUid);
                    // Include uids of deleted entities in the response
                    sendMessage({ id, ...result });
                } else if (type === 'listBooks') {
                    const result = this.listBooks();
                    sendMessage({ id, ...result });
                } else if (type === 'getBookChapters') {
                    const result = this.getBookChapters(payload.bookUid);
                    sendMessage({ id, ...result });
                } else if (type === 'getParagraphs') {
                    const result = this.getParagraphs(payload.chapterUid);
                    sendMessage({ id, ...result });
                } else if (type === 'getParagraph') {
                    const result = this.getParagraph(payload.paragraphUid);
                    sendMessage({ id, ...result });
                } else if (type === 'getParagraphTranslation') {
                    const result = this.getParagraphTranslation(payload.paragraphUid, payload.languageUid);
                    sendMessage({ id, ...result });
                } else if (type === 'getParagraphTranslationShort') {
                    const result = this.getParagraphTranslationShort(payload.paragraphUid, payload.languageUid);
                    sendMessage({ id, ...result });
                } else if (type === 'getNotTranslatedParagraphsUids') {
                    const result = this.getNotTranslatedParagraphsUids(payload.bookUid);
                    sendMessage({ id, ...result });
                } else if (type === 'getWordTranslation') {
                    const result = this.getWordTranslation(payload.wordUid);
                    sendMessage({ id, ...result });
                } else if (type === 'getSentenceTranslation') {
                    const result = this.getSentenceTranslation(payload.sentenceUid);
                    sendMessage({ id, ...result });
                }
            } catch (e: any) {
                port.postMessage({ id, error: e?.message || 'Unknown error' });
            }
        };
    }

    createBookFromText(payload: CreateBookFromTextMessage): { result: UUID, uids: Set<UUID> } {
        const now = Date.now();
        const bookUid = generateUID();
        const path = JSON.stringify(payload.path ?? []);
        const paragraphs = this.splitParagraphs(payload.text);
        const chapterCount = 1; // single implicit chapter
        const paragraphCount = paragraphs.length;
        const translatedParagraphsCount = 0; // none translated at creation time

        const uids = new Set<UUID>();

        this.db.transaction(db => {
            db.exec({
                sql: `INSERT INTO book(uid, path, title, chapterCount, paragraphCount, translatedParagraphsCount, createdAt, updatedAt) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)`,
                bind: [bookUid, path, payload.title, chapterCount, paragraphCount, translatedParagraphsCount, now]
            });
            this.sendUpdateMessage({
                table: "book",
                uid: bookUid,
                action: 'insert',
            });
            uids.add(bookUid);

            // Single implicit chapter index 0
            const chapterUid = generateUID();
            db.exec({
                sql: `INSERT INTO book_chapter(uid, bookUid, chapterIndex, title, createdAt, updatedAt) VALUES(?1, ?2, ?3, ?4, ?5, ?5)`,
                bind: [chapterUid, bookUid, 0, null, now]
            });
            this.sendUpdateMessage({
                table: "book_chapter",
                uid: chapterUid,
                action: 'insert',
            });
            uids.add(chapterUid);

            paragraphs.forEach((p, idx) => {
                const paragraphUid = generateUID();
                db.exec({
                    sql: `INSERT INTO book_chapter_paragraph(uid, chapterUid, paragraphIndex, originalText, originalHtml, createdAt, updatedAt) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?6)`,
                    bind: [paragraphUid, chapterUid, idx, p.text, p.html ?? null, now]
                });
                this.sendUpdateMessage({
                    table: "book_chapter_paragraph",
                    uid: paragraphUid,
                    action: 'insert',
                });
                uids.add(paragraphUid);
            });
        });

        return { result: bookUid, uids };
    }

    private levenshteinDistance(str1: string, str2: string) {
        const track = Array(str2.length + 1)
            .fill(null)
            .map(() => Array(str1.length + 1).fill(null));

        for (let i = 0; i <= str1.length; i += 1) {
            track[0][i] = i;
        }
        for (let j = 0; j <= str2.length; j += 1) {
            track[j][0] = j;
        }

        for (let j = 1; j <= str2.length; j += 1) {
            for (let i = 1; i <= str1.length; i += 1) {
                const indicator = str1[i - 1] === str2[j - 1] ? 0 : 1;
                track[j][i] = Math.min(
                    track[j][i - 1] + 1, // deletion
                    track[j - 1][i] + 1, // insertion
                    track[j - 1][i - 1] + indicator // substitution
                );
            }
        }
        return track[str2.length][str1.length];
    }

    private prepareTranslationObject(paragraphUid: UUID, languageUid: UUID): TranslationDenormal[] {
        const ret: TranslationDenormal[] = [];
        const paragraph = this.getParagraph(paragraphUid).result;
        if (!paragraph) throw new Error("Paragraph not found");

        const originalText = paragraph.originalHtml ?? paragraph.originalText;

        const translation = this.getParagraphTranslation(paragraphUid, languageUid).result;

        if (!translation) throw new Error("Paragraph translation not found");

        let pIdx = 0;
        let sentenceIdx = 0;
        for (const sentence of translation.sentences ?? []) {
            let wordIdx = 0;
            for (const word of sentence.words ?? []) {
                if (word.isPunctuation) {
                    wordIdx++;
                    continue;
                }

                const w = decode(word.original);
                const len = w.length;
                let offset = 0;
                for (; offset < originalText.length - pIdx; offset++) {
                    const pWord = decode(originalText.slice(pIdx + offset, pIdx + offset + len));

                    if (w.length <= 2) {
                        if (w.toLowerCase() === pWord.toLowerCase()) {
                            break;
                        }
                    } else if (this.levenshteinDistance(w.toLowerCase(), pWord.toLowerCase()) < 2) {
                        break;
                    }
                }

                if (offset > 0) {
                    ret.push({
                        text: originalText.slice(pIdx, pIdx + offset)
                    });
                }

                pIdx += offset;

                ret.push({
                    meta: {
                        sentenceTranslationUid: sentence.uid,
                        wordTranslationUid: word.uid,
                        offset,
                    },
                    text: originalText.slice(pIdx, pIdx + len),
                });
                pIdx += len;

                wordIdx++;
            }
            sentenceIdx++;
        }
        if (pIdx < originalText.length) {
            ret.push({ text: originalText.slice(pIdx, originalText.length) });
        }

        return ret;
    }

    // -----------------------
    // Paragraph Translation Support
    // -----------------------
    updateParagraphTranslation(payload: UpdateParagraphTranslationMessage): { result: UUID, uids: Set<UUID> } {
        const now = Date.now();
        const paragraphUid = payload.paragraphUid;
        const languageUid = payload.translation.languageUid;
        // Validate paragraph exists
        const exists = this.db.selectValue("SELECT uid FROM book_chapter_paragraph WHERE uid=?1 LIMIT 1", [paragraphUid]) as UUID | undefined;
        if (!exists) throw new Error("Paragraph not found");

        const uids = new Set<UUID>;
        uids.add(paragraphUid);
        uids.add(languageUid);

        let translationUid = generateUID();
        this.db.transaction(db => {
            // Remove existing translation for (paragraph, language)
            const existingTranslationUid = db.selectValue(
                `SELECT uid FROM book_chapter_paragraph_translation WHERE chapterParagraphUid=?1 AND languageUid=?2 LIMIT 1`,
                [paragraphUid, languageUid]
            ) as UUID | undefined;
            if (existingTranslationUid) {
                // Manually delete dependent words -> sentences -> translation (FKs are RESTRICT)
                const sentenceUids: UUID[] = [];
                db.exec({
                    sql: `SELECT uid FROM book_paragraph_translation_sentence WHERE paragraphTranslationUid = ?1`,
                    bind: [existingTranslationUid],
                    rowMode: 'object',
                    callback: (row: any) => { sentenceUids.push(row.uid as UUID); }
                });

                if (sentenceUids.length > 0) {
                    const placeholders = sentenceUids.map(() => '?').join(',');
                    const wordUids: UUID[] = [];
                    db.exec({
                        sql: `SELECT uid FROM book_paragraph_translation_sentence_word WHERE sentenceUid IN (${placeholders})`,
                        bind: sentenceUids,
                        rowMode: 'object',
                        callback: (row: any) => { wordUids.push(row.uid as UUID); }
                    });
                    if (wordUids.length > 0) {
                        const wp = wordUids.map(() => '?').join(',');
                        db.exec({ sql: `DELETE FROM book_paragraph_translation_sentence_word WHERE uid IN (${wp})`, bind: wordUids });
                        wordUids.forEach(uid => {
                            this.sendUpdateMessage({ table: 'book_paragraph_translation_sentence_word', uid, action: 'delete' });
                            uids.add(uid);
                        });
                    }

                    // Delete sentences next
                    db.exec({ sql: `DELETE FROM book_paragraph_translation_sentence WHERE uid IN (${placeholders})`, bind: sentenceUids });
                    sentenceUids.forEach(uid => {
                        this.sendUpdateMessage({ table: 'book_paragraph_translation_sentence', uid, action: 'delete' });
                        uids.add(uid);
                    });
                }

                // Finally delete the parent translation row
                db.exec({ sql: `DELETE FROM book_chapter_paragraph_translation WHERE uid=?1`, bind: [existingTranslationUid] });
                this.sendUpdateMessage({
                    table: "book_chapter_paragraph_translation",
                    uid: existingTranslationUid,
                    action: 'delete',
                });
                uids.add(existingTranslationUid);
            }

            translationUid = generateUID();
            db.exec({
                sql: `INSERT INTO book_chapter_paragraph_translation(uid, chapterParagraphUid, languageUid, translatingModel, createdAt, updatedAt) VALUES(?1, ?2, ?3, ?4, ?5, ?5)`,
                bind: [translationUid, paragraphUid, languageUid, payload.translation.translatingModel, now]
            });
            this.sendUpdateMessage({
                table: "book_chapter_paragraph_translation",
                uid: translationUid,
                action: 'insert',
            });
            uids.add(translationUid);

            payload.translation.sentences.forEach((sentence, sIdx) => {
                const sentenceUid = generateUID();
                db.exec({
                    sql: `INSERT INTO book_paragraph_translation_sentence(uid, paragraphTranslationUid, sentenceIndex, fullTranslation, createdAt, updatedAt) VALUES(?1, ?2, ?3, ?4, ?5, ?5)`,
                    bind: [sentenceUid, translationUid, sIdx, sentence.fullTranslation, now]
                });
                this.sendUpdateMessage({
                    table: "book_paragraph_translation_sentence",
                    uid: sentenceUid,
                    action: 'insert',
                });
                uids.add(sentenceUid);
                sentence.words.forEach((word, wIdx) => {
                    const wordUid = generateUID();
                    db.exec({
                        sql: `INSERT INTO book_paragraph_translation_sentence_word(uid, sentenceUid, wordIndex, original, isPunctuation, isStandalonePunctuation, isOpeningParenthesis, isClosingParenthesis, wordTranslationUid, wordTranslationInContext, grammarContext, note, createdAt, updatedAt)
                               VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13)`,
                        bind: [
                            wordUid,
                            sentenceUid,
                            wIdx,
                            word.original,
                            word.isPunctuation ? 1 : 0,
                            word.isStandalonePunctuation == null ? null : (word.isStandalonePunctuation ? 1 : 0),
                            word.isOpeningParenthesis == null ? null : (word.isOpeningParenthesis ? 1 : 0),
                            word.isClosingParenthesis == null ? null : (word.isClosingParenthesis ? 1 : 0),
                            word.wordTranslationUid ?? null,
                            word.wordTranslationInContext ? JSON.stringify(word.wordTranslationInContext) : null,
                            word.grammarContext ? JSON.stringify(word.grammarContext) : null,
                            word.note ?? null,
                            now
                        ]
                    });
                    this.sendUpdateMessage({
                        table: "book_paragraph_translation_sentence_word",
                        uid: wordUid,
                        action: 'insert',
                    });
                    uids.add(wordUid);
                });
            });

            // Update translationJson field
            const preparedDenormalizedTranslation = this.prepareTranslationObject(paragraphUid, languageUid);
            db.exec({
                sql: `UPDATE book_chapter_paragraph_translation SET translationJson = ?2 WHERE uid = ?1`,
                bind: [
                    translationUid,
                    JSON.stringify(preparedDenormalizedTranslation),
                ]
            });
            this.sendUpdateMessage({
                table: "book_chapter_paragraph_translation",
                uid: translationUid,
                action: 'update',
            });
            uids.add(translationUid);

            // Recompute translatedParagraphsCount for the parent book (distinct paragraphs having any translation)
            const bookUid = db.selectValue(
                `SELECT b.uid FROM book b
                 JOIN book_chapter bc ON bc.bookUid = b.uid
                 JOIN book_chapter_paragraph p ON p.chapterUid = bc.uid
                 WHERE p.uid = ?1 LIMIT 1`,
                [paragraphUid]
            ) as UUID | undefined;
            if (bookUid) {
                const translatedParagraphsCount = db.selectValue(
                    `SELECT COUNT(DISTINCT p.uid) FROM book_chapter_paragraph p
                     JOIN book_chapter_paragraph_translation t ON t.chapterParagraphUid = p.uid
                     JOIN book_chapter c ON c.uid = p.chapterUid
                     WHERE c.bookUid = ?1`,
                    [bookUid]
                ) as number | null | undefined;
                db.exec({
                    sql: `UPDATE book SET translatedParagraphsCount=?2, updatedAt=?3 WHERE uid=?1`,
                    bind: [bookUid, translatedParagraphsCount ?? 0, now]
                });
                this.sendUpdateMessage({
                    table: "book",
                    uid: bookUid,
                    action: 'update',
                });
                uids.add(bookUid);
            }
        });
        this.sendUpdateMessage({
            table: "book_chapter_paragraph",
            uid: paragraphUid,
            action: "update"
        });
        return { result: translationUid, uids };
    }

    // -----------------------
    // Update Book Path
    // -----------------------
    updateBookPath(payload: UpdateBookPathMessage): { result: UUID, uids: Set<UUID> } {
        const { bookUid } = payload;
        const now = Date.now();
        const exists = this.db.selectValue("SELECT uid FROM book WHERE uid=?1 LIMIT 1", [bookUid]) as UUID | undefined;
        if (!exists) throw new Error('Book not found');
        const pathJson = JSON.stringify(payload.path ?? []);
        this.db.exec({
            sql: `UPDATE book SET path=?2, updatedAt=?3 WHERE uid=?1`,
            bind: [bookUid, pathJson, now]
        });
        this.sendUpdateMessage({
            table: "book",
            uid: bookUid,
            action: 'update',
        });

        const uids = new Set<UUID>();
        uids.add(bookUid);
        return { result: bookUid, uids };
    }

    deleteBook(bookUid: UUID): { result: UUID, uids: Set<UUID> } {
        // Manually cascade delete related records within a single transaction.
        const exists = this.db.selectValue("SELECT uid FROM book WHERE uid=?1 LIMIT 1", [bookUid]) as UUID | undefined;
        if (!exists) throw new Error('Book not found');

        const uids = new Set<UUID>();

        this.db.transaction(db => {
            // Collect chapter UIDs
            const chapterUids: UUID[] = [];
            db.exec({
                sql: `SELECT uid FROM book_chapter WHERE bookUid = ?1`,
                bind: [bookUid],
                rowMode: 'object',
                callback: (row: any) => { chapterUids.push(row.uid as UUID); }
            });

            // Collect paragraph UIDs
            const paragraphUids: UUID[] = [];
            if (chapterUids.length > 0) {
                const placeholders = chapterUids.map(() => '?').join(',');
                db.exec({
                    sql: `SELECT uid FROM book_chapter_paragraph WHERE chapterUid IN (${placeholders})`,
                    bind: chapterUids,
                    rowMode: 'object',
                    callback: (row: any) => { paragraphUids.push(row.uid as UUID); }
                });
            }

            // Collect paragraph translation UIDs
            const translationUids: UUID[] = [];
            if (paragraphUids.length > 0) {
                const placeholders = paragraphUids.map(() => '?').join(',');
                db.exec({
                    sql: `SELECT uid FROM book_chapter_paragraph_translation WHERE chapterParagraphUid IN (${placeholders})`,
                    bind: paragraphUids,
                    rowMode: 'object',
                    callback: (row: any) => { translationUids.push(row.uid as UUID); }
                });
            }

            // Collect sentence UIDs
            const sentenceUids: UUID[] = [];
            if (translationUids.length > 0) {
                const placeholders = translationUids.map(() => '?').join(',');
                db.exec({
                    sql: `SELECT uid FROM book_paragraph_translation_sentence WHERE paragraphTranslationUid IN (${placeholders})`,
                    bind: translationUids,
                    rowMode: 'object',
                    callback: (row: any) => { sentenceUids.push(row.uid as UUID); }
                });
            }

            // Collect word UIDs
            const wordUids: UUID[] = [];
            if (sentenceUids.length > 0) {
                const placeholders = sentenceUids.map(() => '?').join(',');
                db.exec({
                    sql: `SELECT uid FROM book_paragraph_translation_sentence_word WHERE sentenceUid IN (${placeholders})`,
                    bind: sentenceUids,
                    rowMode: 'object',
                    callback: (row: any) => { wordUids.push(row.uid as UUID); }
                });
            }

            const deleteByUids = (table: TableName, ids: UUID[]) => {
                if (!ids.length) return;
                const placeholders = ids.map(() => '?').join(',');
                db.exec({ sql: `DELETE FROM ${table} WHERE uid IN (${placeholders})`, bind: ids });
                ids.forEach(uid => {
                    this.sendUpdateMessage({ table, uid, action: 'delete' });
                    uids.add(uid);
                });
            };

            // Delete deepest-first to satisfy FK RESTRICT constraints
            deleteByUids('book_paragraph_translation_sentence_word', wordUids);
            deleteByUids('book_paragraph_translation_sentence', sentenceUids);
            deleteByUids('book_chapter_paragraph_translation', translationUids);
            deleteByUids('book_chapter_paragraph', paragraphUids);
            deleteByUids('book_chapter', chapterUids);

            // Finally, delete the book
            db.exec({ sql: `DELETE FROM book WHERE uid=?1`, bind: [bookUid] });
            this.sendUpdateMessage({ table: 'book', uid: bookUid, action: 'delete' });
            uids.add(bookUid);
        });

        return { result: bookUid, uids };
    }

    createBookFromEpub(payload: CreateBookFromEpubMessage): { result: UUID, uids: Set<UUID> } {
        const now = Date.now();
        const bookUid = generateUID();
        const path = JSON.stringify(payload.path ?? []);
        const epub = payload.epub;
        const chapterCount = epub.chapters.length;
        const paragraphCount = epub.chapters.reduce((acc, c) => acc + c.paragraphs.length, 0);
        const translatedParagraphsCount = 0; // none translated at creation time

        const uids = new Set<UUID>;

        this.db.transaction(db => {
            db.exec({
                sql: `INSERT INTO book(uid, path, title, chapterCount, paragraphCount, translatedParagraphsCount, createdAt, updatedAt) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)`,
                bind: [bookUid, path, epub.title, chapterCount, paragraphCount, translatedParagraphsCount, now]
            });
            this.sendUpdateMessage({
                table: "book",
                uid: bookUid,
                action: 'insert',
            });
            uids.add(bookUid);

            epub.chapters.forEach((chapter, chapterIndex) => {
                const chapterUid = generateUID();
                db.exec({
                    sql: `INSERT INTO book_chapter(uid, bookUid, chapterIndex, title, createdAt, updatedAt) VALUES(?1, ?2, ?3, ?4, ?5, ?5)`,
                    bind: [chapterUid, bookUid, chapterIndex, chapter.title ?? null, now]
                });
                this.sendUpdateMessage({
                    table: "book_chapter",
                    uid: chapterUid,
                    action: 'insert',
                });
                uids.add(chapterUid);
                chapter.paragraphs.forEach((para, paragraphIndex) => {
                    const paragraphUid = generateUID();
                    db.exec({
                        sql: `INSERT INTO book_chapter_paragraph(uid, chapterUid, paragraphIndex, originalText, originalHtml, createdAt, updatedAt) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?6)`,
                        bind: [paragraphUid, chapterUid, paragraphIndex, para.text, para.html ?? null, now]
                    });
                    this.sendUpdateMessage({
                        table: "book_chapter_paragraph",
                        uid: paragraphUid,
                        action: 'insert',
                    });
                    uids.add(paragraphUid);
                });
            });
        });
        return { result: bookUid, uids };
    }

    // -----------------------
    // List Books
    // -----------------------
    listBooks(): { result: IBookMeta[], uids: Set<UUID> } {
        const rows: { uid: UUID; path: string; title: string; chapterCount: number; paragraphCount: number; translatedParagraphsCount: number; }[] = [];
        this.db.exec({
            sql: `SELECT uid, path, title, chapterCount, paragraphCount, translatedParagraphsCount
                  FROM book
                  ORDER BY updatedAt DESC`,
            rowMode: 'object',
            callback: (row: any) => {
                rows.push(row as any);
            }
        });
        const uids = new Set<UUID>();
        const result = rows.map(r => {
            const paragraphCount = r.paragraphCount;
            const translated = r.translatedParagraphsCount;
            const translationRatio = paragraphCount === 0 ? 0 : (translated / paragraphCount);
            let path: string[] = [];
            try {
                const parsed = JSON.parse(r.path);
                if (Array.isArray(parsed)) path = parsed.filter(p => typeof p === 'string');
            } catch { /* ignore malformed path */ }
            const meta: IBookMeta = {
                uid: r.uid,
                title: r.title,
                chapterCount: r.chapterCount,
                translationRatio,
                path
            };
            uids.add(meta.uid);
            return meta;
        });
        return { result, uids };
    }

    // -----------------------
    // Get Chapters for a Book
    // -----------------------
    getBookChapters(bookUid: UUID): { result: BookChapter[], uids: Set<UUID> } {
        const rows: BookChapter[] = [];
        const uids = new Set<UUID>();
        this.db.exec({
            sql: `SELECT uid, createdAt, updatedAt, title, createdAt, updatedAt
                  FROM book_chapter
                  WHERE bookUid = ?1
                  ORDER BY chapterIndex ASC`,
            bind: [bookUid],
            rowMode: 'object',
            callback: (row: any) => {
                rows.push(row as any);
                uids.add(row.uid);
            }
        });
        return { result: rows, uids };
    }

    getParagraph(paragraphUid: UUID): { result: Paragraph | undefined, uids: Set<UUID> } {
        const r = this.db.selectObject(`
            SELECT uid, createdAt, updatedAt, originalText, originalHtml
            FROM book_chapter_paragraph
            WHERE uid = ?1
            `, [paragraphUid]);
        if (!r) {
            return { result: undefined, uids: new Set() };
        }
        const uid = r["uid"]!.valueOf() as UUID;
        const uids = new Set<UUID>();
        uids.add(uid);
        return {
            result: {
                uid,
                createdAt: r["createdAt"]?.valueOf() as number,
                updatedAt: r["updatedAt"]?.valueOf() as number,
                originalText: r["originalText"] as string,
                originalHtml: r["originalHtml"] as string,
            }, uids
        };
    }

    getParagraphs(chapterUid: UUID): { result: Paragraph[], uids: Set<UUID> } {
        const rows: Paragraph[] = [];
        const uids = new Set<UUID>();
        this.db.exec({
            sql: `SELECT uid, createdAt, updatedAt, originalText, originalHtml
                  FROM book_chapter_paragraph
                  WHERE chapterUid = ?1
                  ORDER BY paragraphIndex ASC`,
            bind: [chapterUid],
            rowMode: 'object',
            callback: (row: any) => {
                rows.push(row as any);
                uids.add(row.uid);
            }
        });
        return { result: rows, uids };
    }

    // -----------------------
    // Get full paragraph translation (latest by updatedAt) including sentences & words
    // -----------------------
    getParagraphTranslation(paragraphUid: UUID, languageUid: UUID): { result: BookParagraphTranslation | undefined, uids: Set<UUID> } {
        const uids = new Set<UUID>();
        uids.add(paragraphUid);
        // Fetch parent translation (pick most recently updated if multiple exist TODO)
        const t = this.db.selectObject(`
            SELECT t.uid as uid, t.createdAt as createdAt, t.updatedAt as updatedAt,
                   t.languageUid as languageUid, l.code as languageCode,
                   t.translatingModel as translatingModel, t.translationJson as translationJson
            FROM book_chapter_paragraph_translation t
            JOIN language l ON l.uid = t.languageUid
            WHERE t.chapterParagraphUid = ?1 AND t.languageUid = ?2
            ORDER BY t.updatedAt DESC
            LIMIT 1
        `, [paragraphUid, languageUid]);
        if (!t) return { result: undefined, uids };

        const translationUid = (t["uid"] as any as UUID);
        uids.add(translationUid);

        // Collect sentences
        const sentences: SentenceTranslation[] = [];
        this.db.exec({
            sql: `SELECT s.uid, s.paragraphTranslationUid, s.sentenceIndex, s.fullTranslation, s.createdAt, s.updatedAt, p.translatingModel
                  FROM book_paragraph_translation_sentence s
                  JOIN book_chapter_paragraph_translation p
                  WHERE s.paragraphTranslationUid = ?1
                  ORDER BY s.sentenceIndex ASC`,
            bind: [translationUid],
            rowMode: 'object',
            callback: (row: any) => {
                sentences.push({
                    uid: row.uid as UUID,
                    createdAt: row.createdAt as number,
                    updatedAt: row.updatedAt as number,
                    paragraphTranslationUid: row.paragraphTranslationUid as UUID,
                    fullTranslation: row.fullTranslation as string,
                    translatingModel: row.translatingModel as ModelId,
                    words: []
                });
                uids.add(row.uid);
            }
        });

        // Map for quick lookup
        const sentenceByUid = new Map<UUID, SentenceTranslation>();
        for (const s of sentences) sentenceByUid.set(s.uid, s);

        // Load words for all sentences in batches of 10
        if (sentences.length > 0) {
            const batchSize = 10;
            for (let start = 0; start < sentences.length; start += batchSize) {
                const batch = sentences.slice(start, start + batchSize);
                const placeholders = batch.map(() => '?').join(',');
                this.db.exec({
                    sql: `SELECT w.uid as uid, w.sentenceUid as sentenceUid, w.wordIndex as wordIndex,
                                 w.original as original, w.isPunctuation as isPunctuation,
                                 w.isStandalonePunctuation as isStandalonePunctuation,
                                 w.isOpeningParenthesis as isOpeningParenthesis,
                                 w.isClosingParenthesis as isClosingParenthesis,
                                 w.wordTranslationUid as wordTranslationUid,
                                 w.wordTranslationInContext as wordTranslationInContext,
                                 w.grammarContext as grammarContext,
                                 w.note as note, w.createdAt as createdAt, w.updatedAt as updatedAt
                          FROM book_paragraph_translation_sentence_word w
                          WHERE w.sentenceUid IN (${placeholders})
                          ORDER BY w.sentenceUid, w.wordIndex ASC`,
                    bind: batch.map(s => s.uid),
                    rowMode: 'object',
                    callback: (row: any) => {
                        const sentence = sentenceByUid.get(row.sentenceUid as UUID);
                        if (!sentence || !sentence.words) return;
                        const word: SentenceWordTranslation = {
                            uid: row.uid as UUID,
                            createdAt: row.createdAt as number,
                            updatedAt: row.updatedAt as number,
                            sentenceUid: row.sentenceUid as UUID,
                            original: row.original as string,
                            isPunctuation: row.isPunctuation ? true : false,
                            isStandalonePunctuation: row.isStandalonePunctuation == null ? null : !!row.isStandalonePunctuation,
                            isOpeningParenthesis: row.isOpeningParenthesis == null ? null : !!row.isOpeningParenthesis,
                            isClosingParenthesis: row.isClosingParenthesis == null ? null : !!row.isClosingParenthesis,
                            wordTranslationUid: row.wordTranslationUid as UUID | undefined,
                            wordTranslationInContext: row.wordTranslationInContext ? JSON.parse(row.wordTranslationInContext) : undefined,
                            grammarContext: row.grammarContext ? JSON.parse(row.grammarContext) : undefined,
                            note: row.note as string | undefined,
                        };
                        sentence.words.push(word);
                        uids.add(row.uid);
                    }
                });
            }
        }

        const ret: BookParagraphTranslation = {
            uid: translationUid,
            createdAt: (t["createdAt"] as any as number),
            updatedAt: (t["updatedAt"] as any as number),
            languageCode: t["languageCode"] as string,
            translatingModel: t["translatingModel"] as ModelId,
            translationJson: t["translationJson"] ? JSON.parse(t["translationJson"] as string) : null,
            sentences,
        };
        return { result: ret, uids };
    }

    // Lightweight fetch: only languageCode + denormalized translationJson without sentences/words expansion
    getParagraphTranslationShort(paragraphUid: UUID, languageUid: UUID): { result: ParagraphTranslationShort | undefined, uids: Set<UUID> } {
        const uids = new Set<UUID>();
        uids.add(paragraphUid);
        const t = this.db.selectObject(`
            SELECT t.uid, l.code as languageCode, t.translationJson as translationJson
            FROM book_chapter_paragraph_translation t
            JOIN language l ON l.uid = t.languageUid
            WHERE t.chapterParagraphUid = ?1 -- AND t.languageUid = ?2
            ORDER BY t.updatedAt DESC
            LIMIT 1
        `, [paragraphUid/*, languageUid*/]); // TODO do not ignore target language
        if (!t) return { result: undefined, uids };
        const translationUid = t["uid"] as UUID;
        uids.add(translationUid);
        return {
            result: {
                languageCode: t["languageCode"] as string,
                translationJson: t["translationJson"] ? JSON.parse(t["translationJson"] as string) : []
            } as ParagraphTranslationShort,
            uids
        };
    }

    // Returns UIDs of paragraphs within a book that have no translations (language-agnostic for now)
    getNotTranslatedParagraphsUids(bookUid: UUID): { result: UUID[], uids: Set<UUID> } {
        const uids = new Set<UUID>();
        const rows: UUID[] = [];
        this.db.exec({
            sql: `SELECT p.uid AS uid
                  FROM book_chapter_paragraph p
                  JOIN book_chapter c ON c.uid = p.chapterUid
                  WHERE c.bookUid = ?1
                    AND NOT EXISTS (
                        SELECT 1 FROM book_chapter_paragraph_translation t
                        WHERE t.chapterParagraphUid = p.uid
                    )`,
            bind: [bookUid],
            rowMode: 'object',
            callback: (row: any) => {
                rows.push(row.uid as UUID);
                uids.add(row.uid);
            }
        });
        // TODO: support filtering by a specific language when multi-language differentiation is needed
        return {result: rows, uids };
    }

    getWordTranslation(wordUid: UUID): { result: SentenceWordTranslation | undefined, uids: Set<UUID> } {
        const uids = new Set<UUID>();
        const r = this.db.selectObject(`
            SELECT uid, sentenceUid, wordIndex, original, isPunctuation, isStandalonePunctuation,
                   isOpeningParenthesis, isClosingParenthesis, wordTranslationUid,
                   wordTranslationInContext, grammarContext, note, createdAt, updatedAt
            FROM book_paragraph_translation_sentence_word
            WHERE uid = ?1
            LIMIT 1
        `, [wordUid]);
        if (!r) return { result: undefined, uids };
        const word: SentenceWordTranslation = {
            uid: r["uid"] as UUID,
            createdAt: r["createdAt"] as number,
            updatedAt: r["updatedAt"] as number,
            sentenceUid: r["sentenceUid"] as UUID,
            original: r["original"] as string,
            isPunctuation: !!r["isPunctuation"],
            isStandalonePunctuation: r["isStandalonePunctuation"] == null ? null : !!r["isStandalonePunctuation"],
            isOpeningParenthesis: r["isOpeningParenthesis"] == null ? null : !!r["isOpeningParenthesis"],
            isClosingParenthesis: r["isClosingParenthesis"] == null ? null : !!r["isClosingParenthesis"],
            wordTranslationUid: r["wordTranslationUid"] as UUID | undefined,
            wordTranslationInContext: r["wordTranslationInContext"] ? JSON.parse(r["wordTranslationInContext"] as string) : undefined,
            grammarContext: r["grammarContext"] ? JSON.parse(r["grammarContext"] as string) : undefined,
            note: r["note"] as string | undefined,
        };
        uids.add(word.uid);
        return { result: word, uids };
    }

    getSentenceTranslation(sentenceUid: UUID): { result: SentenceTranslation | undefined, uids: Set<UUID> } {
        const uids = new Set<UUID>();
        const r = this.db.selectObject(`
            SELECT s.uid as uid, s.paragraphTranslationUid as paragraphTranslationUid,
                   s.fullTranslation as fullTranslation, s.createdAt as createdAt, s.updatedAt as updatedAt, p.translatingModel
            FROM book_paragraph_translation_sentence s
            JOIN book_chapter_paragraph_translation p
            WHERE s.uid = ?1
            LIMIT 1
        `, [sentenceUid]);
        if (!r) return { result: undefined, uids };
        const sentence: SentenceTranslation = {
            uid: r["uid"] as UUID,
            paragraphTranslationUid: r["paragraphTranslationUid"] as UUID,
            createdAt: r["createdAt"] as number,
            updatedAt: r["updatedAt"] as number,
            fullTranslation: r["fullTranslation"] as string,
            translatingModel: r["translatingModel"] as ModelId,
        };
        uids.add(sentence.uid);
        return { result: sentence, uids };
    }
}

// Public message shapes for paragraph translation update
export type UpdateParagraphTranslationMessageGrammar = {
    originalInitialForm: string;
    targetInitialForm: string;
    partOfSpeech: string;
    plurality?: string | null;
    person?: string | null;
    tense?: string | null;
    case?: string | null;
    other?: string | null;
};


export type UpdateParagraphTranslationMessageWord = {
    original: string;
    isPunctuation: boolean;
    isStandalonePunctuation?: boolean | null;
    isOpeningParenthesis?: boolean | null;
    isClosingParenthesis?: boolean | null;
    wordTranslationUid?: UUID;
    wordTranslationInContext?: string[];
    grammarContext?: UpdateParagraphTranslationMessageGrammar;
    note?: string;
};

export type UpdateParagraphTranslationMessageSentence = {
    fullTranslation: string;
    words: UpdateParagraphTranslationMessageWord[];
};

export type UpdateParagraphTranslationMessageTranslation = {
    languageUid: UUID;
    translatingModel: ModelId;
    sentences: UpdateParagraphTranslationMessageSentence[];
}

export type UpdateParagraphTranslationMessage = {
    paragraphUid: UUID;
    translation: UpdateParagraphTranslationMessageTranslation;
};

// Public message shape for updating a book's path
export type UpdateBookPathMessage = {
    bookUid: UUID;
    path?: string[]; // undefined or empty => root
};

