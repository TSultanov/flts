import localforage from "localforage";
import { Translator, type ParagraphTranslation } from "./translator"
import { getConfig } from "./config";
import { GoogleGenAI } from "@google/genai";

export type TextIndex = {
    name: string,
    translationRatio: number;
}

export class Text {
    name: string
    paragraphs: string[]
    translations: Array<ParagraphTranslation | null>

    constructor(name: string, paragraphs: string[]) {
        this.name = name;
        this.paragraphs = paragraphs;
        this.translations = new Array(this.paragraphs.length).fill(null);
    }

    translationRatio() {
        return this.translations.filter(t => t !== null).length / this.paragraphs.length
    }
}

export type TranslationJobStatus = "done" | "pending" | "running" | "failed";

export class TranslationJob {
    private translator: Translator;
    private text: Text;
    private _status: TranslationJobStatus = $state("pending");
    private _ratio: number = $state(0);
    private saveCallback: (text: Text) => Promise<void>;
    private constructor(
        translator: Translator,
        text: Text,
        saveCallback: (text: Text) => Promise<void>,
    ) {
        this.translator = translator;
        this.text = text;
        this.saveCallback = saveCallback;
        this._status = "pending";
    }

    get status() {
        return this._status;
    }

    get name() {
        return this.text.name;
    }

    get ratio() {
        return this._ratio;
    }

    retry() {
        if (this._status === "failed") {
            this.doWork();
        }
    }

    private async doWork() {
        try {
            this._status = "running";
            let idx = 0;

            for (const paragraph in this.text.paragraphs) {
                if (this.text.translations[idx] === null) {
                    let translation = await this.translator.getCachedTranslation({
                        paragraph
                    });

                    if (translation === null) {
                        translation = await this.translator.getTranslation({
                            paragraph
                        });
                    }

                    this.text.translations[idx] = translation;
                    await this.saveCallback(this.text);
                    this._ratio = idx / this.text.paragraphs.length;
                }
                idx++;
            }
            this._status = "done";
            await this.saveCallback(this.text);
        }
        catch (ex) {
            this._status = "failed";
            console.log("Import failed: ", ex);
        }
    }

    static async createTranslationJob(
        text: Text,
        saveCallback: (text: Text) => Promise<void>,
    ) {
        const config = await getConfig();
        const ai = new GoogleGenAI({ apiKey: config.apiKey });
        const translator = await Translator.build(ai, config.targetLanguage);
        const job = new TranslationJob(translator, text, saveCallback);
        job.doWork();
        return job;
    }
}

export class Library {
    private textsStore: LocalForage;
    private indexStore: LocalForage;

    private _index: TextIndex[] = $state([]);
    private _jobs: TranslationJob[] = $state([]);

    constructor() {
        this.textsStore = localforage.createInstance({
            storeName: "library"
        });
        this.indexStore = localforage.createInstance({
            storeName: "index"
        })
    }

    get texts() {
        return this._index;
    }

    get jobs() {
        return this._jobs;
    }

    async loadState() {
        const keys = await this.indexStore.keys();
        const texts: TextIndex[] = [];
        for (const k of keys) {
            const text: TextIndex | null = await this.indexStore.getItem(k);
            if (text) {
                texts.push(text);
            }
        }
        this._index = texts;
    }

    async addText(
        name: string,
        text: string
    ) {
        // First, split the inmput text in paragraphs and clear out empty ones
        const paragraphs: string[] = this.splitParagraphs(text);
        // Save non-translated text
        const t = new Text(name, paragraphs);
        await this.saveText(t);
        // Kick off transaltion job
        const job = await TranslationJob.createTranslationJob(t, (text) => this.saveText(text));
        this._jobs.push(job);
        return job;
    }

    private async saveText(text: Text) {
        await this.textsStore.setItem(text.name, text);
        let index: TextIndex = {
            name: text.name,
            translationRatio: text.translationRatio(),
        }
        await this.indexStore.setItem(text.name, index);
        await this.loadState();
        this._jobs = this._jobs.filter(j => j.status !== "done");
    }

    async deleteText(name: string) {
        this._index = this._index.filter(t => t.name != name);
        await this.indexStore.removeItem(name);
        await this.textsStore.removeItem(name);
    }

    private splitParagraphs(text: string): string[] {
        return text
            .split(/\n+/)
            .map(p => p.trim())
            .filter(p => p.length > 0);
    }
}