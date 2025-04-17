import localforage from "localforage";
import { hashString } from "./utils";

type Word = {
    original: string,
    translations: string[],
    notes: string[],
};

type Sentence = {
    words: Array<Word>,
};

type Paragraph = {
    sentences: Array<Sentence>,
};

export class Dictionary {
    readonly book_hash: string
    readonly store: LocalForage

    constructor(book_hash: string) {
        this.book_hash = book_hash;
        this.store = localforage.createInstance({
            storeName: book_hash,
        })
    }

    async translateParagraph(p: string) {
        let p_hash = await hashString(p);
        let cachedItem = await this.store.getItem(p_hash);
        return cachedItem;
    }
}