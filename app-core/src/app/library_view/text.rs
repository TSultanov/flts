//! Paragraphs as plain text plus UTF-16 ranges, for native text engines.

use library::book::words::ParagraphWords;

use super::{Mark, ParagraphSegment, flatten_original, paragraph_to_segments};

/// A line break inside a paragraph (U+2028), distinct from a paragraph end.
const LINE_SEPARATOR: char = '\u{2028}';

/// A batch of paragraphs plus the book's language (ISO 639-3), which the
/// native reader needs for hyphenation.
#[derive(Clone, serde::Serialize, Debug, PartialEq)]
pub struct ParagraphTexts {
    pub language: String,
    pub paragraphs: Vec<ParagraphText>,
}

#[derive(Clone, serde::Serialize, Debug, PartialEq)]
pub struct ParagraphText {
    pub id: usize,
    pub text: String,
    /// Maximal ranges of uniform emphasis; unmarked text has no run.
    pub runs: Vec<MarkRun>,
    /// Empty for a paragraph not yet translated.
    pub words: Vec<WordRange>,
    pub translated: bool,
}

/// `marks` bit 0 is emphasis, bit 1 strong.
#[derive(Clone, serde::Serialize, Debug, PartialEq)]
pub struct MarkRun {
    pub start: usize,
    pub len: usize,
    pub marks: u8,
}

#[derive(Clone, serde::Serialize, Debug, PartialEq)]
pub struct WordRange {
    pub start: usize,
    pub len: usize,
    #[serde(rename = "s")]
    pub sentence: usize,
    #[serde(rename = "w")]
    pub word: usize,
    #[serde(rename = "f")]
    pub flat_index: usize,
    #[serde(rename = "t", skip_serializing_if = "Option::is_none")]
    pub translation: Option<String>,
}

fn mark_bits(marks: &[Mark]) -> u8 {
    marks.iter().fold(0, |bits, mark| {
        bits | match mark {
            Mark::Emphasis => 1,
            Mark::Strong => 2,
        }
    })
}

struct Builder {
    text: String,
    utf16: usize,
    runs: Vec<MarkRun>,
}

impl Builder {
    fn push(&mut self, piece: &str, marks: u8) -> (usize, usize) {
        let start = self.utf16;
        let len: usize = piece.encode_utf16().count();
        self.text.push_str(piece);
        self.utf16 += len;
        if marks != 0 && len > 0 {
            match self.runs.last_mut() {
                Some(run) if run.marks == marks && run.start + run.len == start => run.len += len,
                _ => self.runs.push(MarkRun { start, len, marks }),
            }
        }
        (start, len)
    }

    /// Text between words. A literal newline is only white space in HTML: it
    /// reads as a space, and as nothing right after a break or other space.
    fn push_text(&mut self, piece: &str, marks: u8) {
        if !piece.contains(['\n', '\r']) {
            self.push(piece, marks);
            return;
        }
        let mut buf = [0u8; 4];
        for ch in piece.chars() {
            if ch == '\n' || ch == '\r' {
                let after_space = self
                    .text
                    .chars()
                    .next_back()
                    .is_none_or(|c| c == LINE_SEPARATOR || c.is_whitespace());
                if !after_space {
                    self.push(" ", marks);
                }
            } else {
                self.push(ch.encode_utf8(&mut buf), marks);
            }
        }
    }

    /// Drops line breaks that end the paragraph. HTML shows no line for a
    /// closing `<br>`, while a line separator would add an empty one.
    fn trim_trailing_breaks(&mut self) {
        let kept = self
            .text
            .trim_end_matches(|c: char| c == LINE_SEPARATOR || c.is_whitespace());
        if !self.text[kept.len()..].contains(LINE_SEPARATOR) {
            return;
        }
        self.text.truncate(kept.len());
        self.utf16 = self.text.encode_utf16().count();
        let end = self.utf16;
        self.runs.retain_mut(|run| {
            run.len = run.len.min(end.saturating_sub(run.start));
            run.len > 0
        });
    }
}

pub fn paragraph_text(id: usize, original: &str, words: Option<&ParagraphWords>) -> ParagraphText {
    let mut b = Builder {
        text: String::with_capacity(original.len()),
        utf16: 0,
        runs: Vec::new(),
    };
    let mut ranges = Vec::new();
    let mut buf = [0u8; 4];

    match words {
        Some(words) => {
            for segment in paragraph_to_segments(original, words) {
                match segment {
                    ParagraphSegment::Gap { text, marks } => {
                        b.push_text(&text, mark_bits(&marks));
                    }
                    ParagraphSegment::Break { marks } => {
                        b.push(LINE_SEPARATOR.encode_utf8(&mut buf), mark_bits(&marks));
                    }
                    ParagraphSegment::Word {
                        text,
                        marks,
                        sentence,
                        word,
                        flat_index,
                        translation,
                    } => {
                        let (start, len) = b.push(&text, mark_bits(&marks));
                        ranges.push(WordRange {
                            start,
                            len,
                            sentence,
                            word,
                            flat_index,
                            translation,
                        });
                    }
                }
            }
        }
        None => {
            let chars: Vec<char> = original.chars().collect();
            for c in flatten_original(&chars) {
                if c.is_break {
                    b.push(LINE_SEPARATOR.encode_utf8(&mut buf), mark_bits(&c.marks));
                } else {
                    b.push_text(c.ch.encode_utf8(&mut buf), mark_bits(&c.marks));
                }
            }
        }
    }

    b.trim_trailing_breaks();
    ParagraphText {
        id,
        text: b.text,
        runs: b.runs,
        translated: words.is_some(),
        words: ranges,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_literal_newline_is_white_space() {
        // Verse as importers write it: a break, then a newline in the source.
        let p = paragraph_text(1, "one, <br>\ntwo<br>\r\nthree", None);
        assert_eq!(p.text, "one, \u{2028}two\u{2028}three");
        assert_eq!(
            paragraph_text(1, "wrapped\nline", None).text,
            "wrapped line"
        );
        assert_eq!(
            paragraph_text(1, "wrapped \nline", None).text,
            "wrapped line"
        );
    }

    #[test]
    fn a_closing_break_adds_no_empty_line() {
        let p = paragraph_text(1, "verse line<br>", None);
        assert_eq!(p.text, "verse line");
        let p = paragraph_text(1, "one<br>two <b><br></b>", None);
        assert_eq!(p.text, "one\u{2028}two");
        assert!(p.runs.is_empty());
        // Without a break, trailing space is left alone.
        assert_eq!(paragraph_text(1, "plain ", None).text, "plain ");
    }

    #[test]
    fn untranslated_original_is_decoded_with_marks_and_breaks() {
        let p = paragraph_text(3, "a <i>b&amp;c</i><br>d", None);
        assert_eq!(p.text, "a b&c\u{2028}d");
        assert_eq!(
            p.runs,
            vec![MarkRun {
                start: 2,
                len: 3,
                marks: 1
            }]
        );
        assert!(p.words.is_empty());
        assert!(!p.translated);
    }

    #[test]
    fn offsets_count_utf16_units() {
        let p = paragraph_text(0, "😀 <b>x</b>", None);
        assert_eq!(
            p.runs,
            vec![MarkRun {
                start: 3,
                len: 1,
                marks: 2
            }]
        );
    }
}
