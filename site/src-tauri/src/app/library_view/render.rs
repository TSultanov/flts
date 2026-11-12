//! A translated paragraph as the HTML the page shows, made here so the page
//! has it to insert and not a component to build for every word.

use std::fmt::Write;

use super::{Mark, ParagraphSegment};

/// The inside of the paragraph's `<p>`. What a word carries for the page:
/// where it is (`data-s`, `data-w`, `data-flat-index`) and its translation
/// (`data-t`), for a tap to show.
pub fn paragraph_html(segments: &[ParagraphSegment]) -> String {
    let mut html = String::new();
    for segment in segments {
        let marks = match segment {
            ParagraphSegment::Gap { marks, .. }
            | ParagraphSegment::Break { marks }
            | ParagraphSegment::Word { marks, .. } => marks,
        };
        // A break is never wrapped, as the page never wrapped it.
        if let ParagraphSegment::Break { .. } = segment {
            html.push_str("<br>");
            continue;
        }
        let strong = marks.contains(&Mark::Strong);
        let emphasis = marks.contains(&Mark::Emphasis);
        if strong {
            html.push_str("<strong>");
        }
        if emphasis {
            html.push_str("<em>");
        }
        match segment {
            ParagraphSegment::Gap { text, .. } => escape(&mut html, text, false),
            ParagraphSegment::Word {
                text,
                sentence,
                word,
                flat_index,
                translation,
                ..
            } => {
                let _ = write!(
                    html,
                    r#"<span class="word-span" data-s="{sentence}" data-w="{word}" data-flat-index="{flat_index}""#
                );
                let translation = translation.as_deref().filter(|t| !t.is_empty());
                if let Some(translation) = translation {
                    html.push_str(r#" data-t=""#);
                    escape(&mut html, translation, true);
                    html.push('"');
                }
                html.push('>');
                escape(&mut html, text, false);
                html.push_str("</span>");
            }
            ParagraphSegment::Break { .. } => {}
        }
        if emphasis {
            html.push_str("</em>");
        }
        if strong {
            html.push_str("</strong>");
        }
    }
    html
}

fn escape(html: &mut String, text: &str, attribute: bool) {
    for c in text.chars() {
        match c {
            '&' => html.push_str("&amp;"),
            '<' => html.push_str("&lt;"),
            '>' => html.push_str("&gt;"),
            '"' if attribute => html.push_str("&quot;"),
            c => html.push(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(text: &str, translation: Option<&str>) -> ParagraphSegment {
        ParagraphSegment::Word {
            text: text.into(),
            marks: vec![],
            sentence: 1,
            word: 2,
            flat_index: 3,
            translation: translation.map(str::to_owned),
        }
    }

    #[test]
    fn a_word_says_where_it_is_and_what_it_means() {
        assert_eq!(
            paragraph_html(&[word("Haus", Some("house"))]),
            r#"<span class="word-span" data-s="1" data-w="2" data-flat-index="3" data-t="house">Haus</span>"#
        );
    }

    #[test]
    fn a_word_with_no_translation_has_none_to_show() {
        let html = paragraph_html(&[word("und", None), word("so", Some(""))]);
        assert!(!html.contains("data-t"));
    }

    #[test]
    fn text_cannot_become_markup() {
        let html = paragraph_html(&[
            word("a<b", Some(r#"x"&<y"#)),
            ParagraphSegment::Gap {
                text: " & <i> ".into(),
                marks: vec![],
            },
        ]);
        assert!(html.contains(r#"data-t="x&quot;&amp;&lt;y""#));
        assert!(html.contains(r#">a&lt;b</span>"#));
        assert!(html.ends_with(" &amp; &lt;i&gt; "));
    }

    #[test]
    fn marks_wrap_what_they_mark_strong_outside_emphasis() {
        let html = paragraph_html(&[
            ParagraphSegment::Gap {
                text: "a".into(),
                marks: vec![Mark::Emphasis, Mark::Strong],
            },
            ParagraphSegment::Break {
                marks: vec![Mark::Strong],
            },
            ParagraphSegment::Gap {
                text: "b".into(),
                marks: vec![Mark::Emphasis],
            },
        ]);
        assert_eq!(html, "<strong><em>a</em></strong><br><em>b</em>");
    }
}
