import {
	FiniteNumber,
	id,
	object,
	json,
	array,
	nullOr,
	SqliteBoolean,
	String as TString,
} from "@evolu/common";

// Branded IDs per table
export const LanguageId = id("Language");
export type LanguageId = typeof LanguageId.Type;

export const WordId = id("Word");
export type WordId = typeof WordId.Type;

export const WordSpellingVariantId = id("WordSpellingVariantId");
export type WordSpellingVariantId = typeof WordSpellingVariantId.Type;

export const WordTranslationSpellingVariantId = id("WordTranslationSpellingVariantId");
export type WordTranslationSpellingVariantId = typeof WordTranslationSpellingVariantId.Type;

export const WordTranslationId = id("WordTranslation");
export type WordTranslationId = typeof WordTranslationId.Type;

export const BookId = id("Book");
export type BookId = typeof BookId.Type;

export const BookChapterId = id("BookChapter");
export type BookChapterId = typeof BookChapterId.Type;

export const BookChapterParagraphId = id("BookChapterParagraph");
export type BookChapterParagraphId = typeof BookChapterParagraphId.Type;

export const BookParagraphTranslationId = id("BookParagraphTranslation");
export type BookParagraphTranslationId = typeof BookParagraphTranslationId.Type;

export const BookParagraphTranslationSentenceId = id(
	"BookParagraphTranslationSentence",
);
export type BookParagraphTranslationSentenceId =
	typeof BookParagraphTranslationSentenceId.Type;

export const BookParagraphTranslationSentenceWordId = id(
	"BookParagraphTranslationSentenceWord",
);
export type BookParagraphTranslationSentenceWordId =
	typeof BookParagraphTranslationSentenceWordId.Type;

// Typed JSON for grammar context used in sentence words
export const Grammar = object({
	originalInitialForm: TString,
	targetInitialForm: TString,
	partOfSpeech: TString,
	plurality: nullOr(TString),
	person: nullOr(TString),
	tense: nullOr(TString),
	case: nullOr(TString),
	other: nullOr(TString),
});
export type Grammar = typeof Grammar.Type;
export const GrammarJson = json(Grammar, "GrammarJson");
export type GrammarJson = typeof GrammarJson.Type;

// Typed JSON for array of strings used in wordTranslationInContext
export const StringArray = array(TString);
export type StringArray = typeof StringArray.Type;
export const StringArrayJson = json(StringArray, "StringArrayJson");
export type StringArrayJson = typeof StringArrayJson.Type;

// Evolu Schema equivalent to src/lib/data/sql/migrations.ts
// Notes:
// - SQLite indexes and foreign keys are not modeled at the Type level; relations
//   are expressed via ID columns.
export const Schema = {
	// language(uid, code)
	language: {
		id: LanguageId,
		code: TString,
	},

	// word(uid, originalLanguageUid -> language.id, original)
	word: {
		id: WordId,
		originalLanguageId: LanguageId,
	},

	wordSpellingVariant: {
		id: WordSpellingVariantId,
		wordId: WordId,
	},

	// word_translation(uid, translationLanguageUid -> language.id, originalWordUid -> word.id, translation)
	wordTranslation: {
		id: WordTranslationId,
		translationLanguageId: LanguageId,
		originalWordVariantId: WordSpellingVariantId,
	},

	wordTranslationSpellingVariant: {
		id: WordTranslationSpellingVariantId,
		wordTranslationId: WordTranslationId,
	},

	// book(uid, path JSON string[], title, chapterCount, paragraphCount, translatedParagraphsCount)
	book: {
		id: BookId,
		path: StringArrayJson,
		title: TString,
	},

	// book_chapter(uid, bookUid -> book.id, chapterIndex, title nullable)
	bookChapter: {
		id: BookChapterId,
		bookId: BookId,
		chapterIndex: FiniteNumber,
		title: nullOr(TString),
	},

	// book_chapter_paragraph(uid, chapterUid -> book_chapter.id, paragraphIndex, originalText, originalHtml nullable)
	bookChapterParagraph: {
		id: BookChapterParagraphId,
		chapterId: BookChapterId,
		paragraphIndex: FiniteNumber,
		originalText: TString,
		originalHtml: nullOr(TString),
	},

	// book_chapter_paragraph_translation(uid, chapterParagraphUid -> book_chapter_paragraph.id, languageUid -> language.id, translatingModel)
	bookParagraphTranslation: {
		id: BookParagraphTranslationId,
		chapterParagraphId: BookChapterParagraphId,
		languageId: LanguageId,
		translatingModel: TString,
	},

	// book_paragraph_translation_sentence(uid, paragraphTranslationUid -> book_paragraph_translation.id, sentenceIndex, fullTranslation)
	bookParagraphTranslationSentence: {
		id: BookParagraphTranslationSentenceId,
		paragraphTranslationId: BookParagraphTranslationId,
		sentenceIndex: FiniteNumber,
		fullTranslation: TString,
	},

	// book_paragraph_translation_sentence_word(uid, sentenceUid -> book_paragraph_translation_sentence.id, wordIndex, ...flags, wordTranslationUid nullable, wordTranslationInContext nullable, grammarContext nullable JSON, note nullable)
	bookParagraphTranslationSentenceWord: {
		id: BookParagraphTranslationSentenceWordId,
		sentenceId: BookParagraphTranslationSentenceId,
		wordIndex: FiniteNumber,
		original: TString,
		isPunctuation: SqliteBoolean,
		wordTranslationId: WordTranslationId,
		wordTranslationInContext: StringArrayJson,
		grammarContext: GrammarJson,
		note: nullOr(TString),
	},
} as const;

export type DatabaseSchema = typeof Schema;

