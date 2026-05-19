import type { UUID } from "./uuid";

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

export type SentenceWordTranslation = {
    readonly original: string,
    readonly note: string,
    readonly isPunctuation: boolean,
    readonly contextualTranslations: string[],
    readonly grammar: Grammar,
    readonly fullSentenceTranslation: string,
    readonly translationModel: number,
    readonly sourceLanguage: string,
}

export type ParagraphSegment =
    | { kind: "gap", html: string }
    | {
          kind: "word",
          text: string,
          sentence: number,
          word: number,
          flatIndex: number,
          translation: string | null,
      }

export type ParagraphOriginal = {
    id: number,
    original: string,
}

export type ParagraphTranslationSlice = {
    id: number,
    segments?: ParagraphSegment[],
    visibleWords: number[],
}

export type ChapterMetaView = {
    id: number,
    title: string,
}

export interface BookMeta {
    readonly uid: UUID,
    readonly chapterCount: number;
    readonly translationRatio: number;
    readonly title: string;
    path: string[];
}
