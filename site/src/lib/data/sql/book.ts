import { readable, type Readable } from "svelte/store";
import { generateUID, type UUID } from "../v2/db";

type BookData = {
    path: string[];
    readonly title: string,
    readonly chapterCount: number;
    readonly paragraphCount: number;
    readonly translatedParagraphsCount: number;
}

export type BookEntity = BookData

type BookChapter ={
    readonly title?: string,
}

export type Paragraph = {
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

export type BookParagraphTranslation = {
    readonly languageCode: string,
    readonly translatingModel: string,
    readonly sentences?: SentenceTranslation[],
}

export type SentenceTranslation = {
    readonly paragraphTranslationUid: UUID,
    readonly translatingModel: string,
    readonly fullTranslation: string,
    readonly words?: SentenceWordTranslation[],
}

export type SentenceWordTranslation = {
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
