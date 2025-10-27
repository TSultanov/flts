export type Grammar = {
    originalInitialForm: string,
    targetInitialForm: string,
    partOfSpeech: string
    plurality?: string | null,
    person?: string | null,
    tense?: string | null,
    case?: string | null,
    other?: string | null,
}

export type WordTranslation = {
    original: string,
    isPunctuation: boolean,
    isStandalonePunctuation?: boolean | null,
    isOpeningParenthesis?: boolean | null,
    isClosingParenthesis?: boolean | null,
    translations: string[],
    note: string,
    grammar: Grammar,
};

export type SentenceTranslation = {
    words: WordTranslation[],
    fullTranslation: string,
}

export type ParagraphTranslation = {
    sentences: SentenceTranslation[],
    sourceLanguage: string,
    targetLanguage: string,
}

export type DictionaryRequest = {
    paragraph: string,
}

export interface Translator {
    getCachedTranslation(p: DictionaryRequest): Promise<ParagraphTranslation | null>,
    getTranslation(p: DictionaryRequest): Promise<ParagraphTranslation>
}
