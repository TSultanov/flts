import localforage from "localforage"
import ePub from "../../vendor/epub-js/src/epub";
import type Packaging from "../../vendor/epub-js/src/packaging";
import { hashBuffer } from "./utils";
import { Dictionary } from "./dictionary";
import type { GoogleGenAI } from "@google/genai";

export class Book {
    metadata: BookRecord
    private store: LocalForage
    constructor(store: LocalForage, metadata: BookRecord) {
        this.metadata = metadata;
        this.store = store;
    }

    async getContent(): Promise<ArrayBuffer | null> {
        return await this.store.getItem(`bookdata_${this.metadata.bookHash}`);
    }

    async updateCfi(cfi: string) {
        const meta: BookRecord | null = await this.store.getItem(`bookmeta_${this.metadata.bookHash}`);
        if (!meta) {
            console.error(`Book ${this.metadata.bookHash} was lost`);
            return;
        }
        const updatedMeta = { ...meta, lastCfi: cfi };
        await this.store.setItem(`bookmeta_${this.metadata.bookHash}`, updatedMeta);
    }

    async getDictionary(ai: GoogleGenAI) {
        return await Dictionary.build(ai, this.metadata.bookHash, "English", "Russian")
    }
}

export type BookRecord = {
    bookHash: string,
    author: string
    title: string,
    lastCfi?: string
}

export class Library {
    private store: LocalForage
    private catalog: Set<string>

    constructor(store: LocalForage, catalog: Set<string>) {
        this.store = store;
        this.catalog = catalog;
    }

    static async build() {
        const store = localforage.createInstance({
            storeName: "library"
        })
        const catalog: Set<string> = await store.getItem('catalog') ?? new Set();
        return new Library(store, catalog);
    }

    async getCatalog() {
        const items: Book[] = [];
        for (const hash of this.catalog) {
            const item: BookRecord | null = await this.store.getItem(`bookmeta_${hash}`);
            if (item) {
                const book = new Book(this.store, item);
                items.push(book);
            }
        }
        items.sort((a, b) => a.metadata.author.localeCompare(b.metadata.author) || a.metadata.title.localeCompare(b.metadata.title))
        return items;
    }

    async addBook(data: ArrayBuffer) {
        const epub = ePub(data);
        const bookHash = await hashBuffer(data);
        await epub.opened;

        const packaging: Packaging = await epub.loaded?.packaging;
        const metadata = packaging.metadata;

        const record = {
            bookHash,
            data,
            author: metadata.get('creator'),
            title: metadata.get('title')
        }

        await this.storeBookData(record, data);
        this.catalog.add(record.bookHash);
        await this.storeCatalog();
    }

    async removeBook(hash: string) {
        this.catalog.delete(hash);
        await this.storeCatalog();
        await this.removeBookData(hash);
    }

    private async storeCatalog() {
        await this.store.setItem('catalog', this.catalog);
    }

    private async storeBookData(record: BookRecord, data: ArrayBuffer) {
        const hash = record.bookHash;
        await this.store.setItem(`bookdata_${hash}`, data);
        await this.store.setItem(`bookmeta_${hash}`, record);
    }

    private async removeBookData(hash: string) {
        await this.store.removeItem(`bookmeta_${hash}`);
        await this.store.removeItem(`bookdata_${hash}`);
    }
}