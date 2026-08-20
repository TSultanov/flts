use std::path::Path;

use epub::doc::EpubDoc;
use scraper::{ElementRef, Html, Node, Selector};
use serde::{Deserialize, Serialize};

const ALLOWED_TAGS: &[&str] = &["em", "i", "b", "br"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpubBook {
    pub title: String,
    pub chapters: Vec<EpubChapter>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpubChapter {
    pub title: String,
    pub paragraphs: Vec<EpubParagraph>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpubParagraph {
    pub text: String,
    pub html: String,
}

impl EpubBook {
    pub fn load(path: &Path) -> anyhow::Result<EpubBook> {
        Self::from_doc(&mut EpubDoc::new(path)?)
    }

    pub fn from_bytes(bytes: Vec<u8>) -> anyhow::Result<EpubBook> {
        Self::from_doc(&mut EpubDoc::from_reader(std::io::Cursor::new(bytes))?)
    }

    fn from_doc<R: std::io::Read + std::io::Seek>(
        epub: &mut EpubDoc<R>,
    ) -> anyhow::Result<EpubBook> {
        let mut chapters = Vec::new();

        let spine_items = epub.spine.clone();
        let toc_items = epub.toc.clone();

        for spine_item in &spine_items {
            // Skip spine items whose manifest resource is missing or has a
            // non-UTF-8 path, so a bad EPUB can't panic the import.
            let Some(resource) = epub.resources.get(&spine_item.idref) else {
                continue;
            };
            let Some(resource_path) = resource.path.to_str() else {
                continue;
            };
            let c_href_doc = resource_path.replace("OEBPS/", "");

            let toc_elements: Vec<_> = toc_items
                .iter()
                .filter(|t| {
                    let t_href_doc = t
                        .content
                        .to_string_lossy()
                        .split('#')
                        .next()
                        .unwrap_or("")
                        .replace("OEBPS/", "");
                    t_href_doc == c_href_doc
                })
                .collect();

            if let Some((content, _)) = epub.get_resource_str(&spine_item.idref) {
                chapters.extend(parse_chapter(&content, &toc_elements)?);
            }
        }

        let mut title_parts = Vec::new();
        if let Some(creator) = epub.mdata("creator")
            && !creator.value.is_empty()
        {
            title_parts.push(creator.value.clone());
        }
        if let Some(title) = epub.mdata("title")
            && !title.value.is_empty()
        {
            title_parts.push(title.value.clone());
        }

        let language = epub
            .mdata("language")
            .map(|m| m.value.trim().to_string())
            .filter(|s| !s.is_empty());

        Ok(EpubBook {
            title: title_parts.join(" - "),
            chapters,
            language,
        })
    }
}

fn parse_chapter(
    chapter_html: &str,
    toc: &[&epub::doc::NavPoint],
) -> anyhow::Result<Vec<EpubChapter>> {
    let document = Html::parse_document(chapter_html);

    if toc.is_empty() {
        return Ok(vec![EpubChapter {
            title: extract_title(&document),
            paragraphs: text_between_anchors(&document, "", None)?,
        }]);
    }

    let mut chapters = Vec::new();

    for (i, t_curr) in toc.iter().enumerate() {
        let t_next = if i + 1 < toc.len() {
            Some(toc[i + 1])
        } else {
            None
        };

        let start_anchor = split_anchor(&t_curr.content.to_string_lossy());
        let end_anchor = t_next.map(|t| split_anchor(&t.content.to_string_lossy()));

        let paragraphs = text_between_anchors(&document, &start_anchor, end_anchor.as_deref())?;

        chapters.push(EpubChapter {
            title: t_curr.label.clone(),
            paragraphs,
        });
    }

    Ok(chapters)
}

fn split_anchor(href: &str) -> String {
    href.split('#').nth(1).unwrap_or("").to_string()
}

fn extract_title(document: &Html) -> String {
    let title_selector = Selector::parse("title").unwrap();
    if let Some(title_element) = document.select(&title_selector).next() {
        title_element.text().collect::<String>()
    } else {
        String::new()
    }
}

fn text_between_anchors(
    document: &Html,
    anchor1: &str,
    anchor2: Option<&str>,
) -> anyhow::Result<Vec<EpubParagraph>> {
    let start_element = if anchor1.is_empty() {
        find_body_element(document)
    } else {
        find_element_by_id(document, anchor1)
    };

    let end_element = anchor2.and_then(|a| find_element_by_id(document, a));

    if let Some(start) = start_element {
        Ok(text_between(start, end_element))
    } else {
        Ok(Vec::new())
    }
}

fn find_body_element(document: &Html) -> Option<ElementRef<'_>> {
    let body_selector = Selector::parse("body").unwrap();
    document.select(&body_selector).next()
}

fn find_element_by_id<'a>(document: &'a Html, id: &str) -> Option<ElementRef<'a>> {
    let id_selector = Selector::parse(&format!("[id=\"{}\"]", id)).ok()?;
    document.select(&id_selector).next()
}

fn all_children_are_inline(element: ElementRef) -> bool {
    for child in element.children() {
        if let Some(child_element) = ElementRef::wrap(child)
            && !is_inline_element(child_element.value().name())
        {
            return false;
        }
    }
    true
}

fn is_inline_element(tag_name: &str) -> bool {
    // eq_ignore_ascii_case avoids allocating a lowercased copy.
    matches!(
        tag_name,
        "a" | "A"
            | "abbr"
            | "ABBR"
            | "b"
            | "B"
            | "bdi"
            | "BDI"
            | "bdo"
            | "BDO"
            | "br"
            | "BR"
            | "cite"
            | "CITE"
            | "code"
            | "CODE"
            | "data"
            | "DATA"
            | "dfn"
            | "DFN"
            | "em"
            | "EM"
            | "i"
            | "I"
            | "kbd"
            | "KBD"
            | "mark"
            | "MARK"
            | "q"
            | "Q"
            | "s"
            | "S"
            | "samp"
            | "SAMP"
            | "small"
            | "SMALL"
            | "span"
            | "SPAN"
            | "strong"
            | "STRONG"
            | "sub"
            | "SUB"
            | "sup"
            | "SUP"
            | "time"
            | "TIME"
            | "u"
            | "U"
            | "var"
            | "VAR"
    )
}

fn text_between(start: ElementRef, end: Option<ElementRef>) -> Vec<EpubParagraph> {
    let mut paragraphs = Vec::new();
    let mut current = Some(start);

    while let Some(elem) = current {
        if let Some(end_elem) = end
            && elem.id() == end_elem.id()
        {
            break;
        }

        let has_text = elem.text().any(|t| !t.trim().is_empty());
        if has_text && (elem.children().count() == 0 || all_children_are_inline(elem)) {
            let text = elem.text().collect::<String>().trim().to_string();
            if !text.is_empty() {
                let html = get_sanitized_html(elem, false).trim().to_string();
                paragraphs.push(EpubParagraph { text, html });
            }
        }

        // Children first, then siblings, then up to the parent's sibling.
        if !all_children_are_inline(elem)
            && let Some(first_child) = elem.children().find_map(ElementRef::wrap)
        {
            current = Some(first_child);
            continue;
        }

        current = find_next_sibling(elem).or_else(|| {
            let mut parent = elem.parent();
            while let Some(p_node) = parent {
                if let Some(p) = ElementRef::wrap(p_node) {
                    if let Some(next) = find_next_sibling(p) {
                        return Some(next);
                    }
                    parent = p.parent();
                } else {
                    break;
                }
            }
            None
        });
    }

    paragraphs
}

fn find_next_sibling(element: ElementRef) -> Option<ElementRef> {
    let mut next = element.next_sibling();
    while let Some(node) = next {
        if let Some(elem) = ElementRef::wrap(node) {
            return Some(elem);
        }
        next = node.next_sibling();
    }
    None
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn get_sanitized_html(element: ElementRef, keep_bounding_tag: bool) -> String {
    let tag_lower = element.value().name().to_lowercase();

    if keep_bounding_tag && !ALLOWED_TAGS.contains(&tag_lower.as_str()) {
        return escape_html(&element.text().collect::<String>());
    }

    if tag_lower == "br" {
        return "<br>".to_string();
    }

    let mut html = if keep_bounding_tag {
        format!("<{}>", tag_lower)
    } else {
        String::new()
    };

    for child in element.children() {
        match child.value() {
            Node::Element(_) => {
                if let Some(child_elem) = ElementRef::wrap(child) {
                    html.push_str(&get_sanitized_html(child_elem, true));
                }
            }
            Node::Text(text) => {
                html.push_str(&escape_html(text));
            }
            _ => {}
        }
    }

    if keep_bounding_tag {
        html.push_str(&format!("</{}>", tag_lower));
    }

    html
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipWriter};

    fn escape_xml(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&#39;")
    }

    /// Minimal EPUB matching `site/tests/fixtures/epub-generator.ts`.
    fn build_epub(
        title: &str,
        author: &str,
        language: Option<&str>,
        chapters: &[(&str, &str)],
    ) -> Vec<u8> {
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

        zip.start_file("mimetype", stored).unwrap();
        zip.write_all(b"application/epub+zip").unwrap();

        let container = r#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#;
        zip.start_file("META-INF/container.xml", deflated).unwrap();
        zip.write_all(container.as_bytes()).unwrap();

        let language_xml = match language {
            Some(lang) => format!("<dc:language>{}</dc:language>", escape_xml(lang)),
            None => String::new(),
        };

        let manifest_chapters = chapters
            .iter()
            .enumerate()
            .map(|(i, _)| {
                format!(
                    r#"<item id="chapter{n}" href="chapter{n}.xhtml" media-type="application/xhtml+xml"/>"#,
                    n = i + 1
                )
            })
            .collect::<Vec<_>>()
            .join("\n    ");
        let spine_chapters = chapters
            .iter()
            .enumerate()
            .map(|(i, _)| format!(r#"<itemref idref="chapter{}"/>"#, i + 1))
            .collect::<Vec<_>>()
            .join("\n    ");

        let content_opf = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" unique-identifier="bookid" version="2.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">
    <dc:title>{title}</dc:title>
    <dc:creator>{author}</dc:creator>
    <dc:identifier id="bookid">test-book</dc:identifier>
    {language_xml}
  </metadata>
  <manifest>
    <item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>
    <item id="toc" href="toc.xhtml" media-type="application/xhtml+xml"/>
    {manifest_chapters}
  </manifest>
  <spine toc="ncx">
    <itemref idref="toc"/>
    {spine_chapters}
  </spine>
</package>"#,
            title = escape_xml(title),
            author = escape_xml(author),
        );
        zip.start_file("OEBPS/content.opf", deflated).unwrap();
        zip.write_all(content_opf.as_bytes()).unwrap();

        let nav_points = chapters
            .iter()
            .enumerate()
            .map(|(i, (ch_title, _))| {
                format!(
                    r#"
    <navPoint id="navpoint-{n}" playOrder="{order}">
      <navLabel>
        <text>{label}</text>
      </navLabel>
      <content src="chapter{n}.xhtml"/>
    </navPoint>"#,
                    n = i + 1,
                    order = i + 2,
                    label = escape_xml(ch_title),
                )
            })
            .collect::<Vec<_>>()
            .join("");

        let toc_ncx = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE ncx PUBLIC "-//NISO//DTD ncx 2005-1//EN" "http://www.daisy.org/z3986/2005/ncx-2005-1.dtd">
<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1">
  <head>
    <meta name="dtb:uid" content="test-book"/>
    <meta name="dtb:depth" content="1"/>
    <meta name="dtb:totalPageCount" content="0"/>
    <meta name="dtb:maxPageNumber" content="0"/>
  </head>
  <docTitle>
    <text>{title}</text>
  </docTitle>
  <navMap>
    <navPoint id="navpoint-toc" playOrder="1">
      <navLabel>
        <text>Table of Contents</text>
      </navLabel>
      <content src="toc.xhtml"/>
    </navPoint>
    {nav_points}
  </navMap>
</ncx>"#,
            title = escape_xml(title),
        );
        zip.start_file("OEBPS/toc.ncx", deflated).unwrap();
        zip.write_all(toc_ncx.as_bytes()).unwrap();

        let toc_links = chapters
            .iter()
            .enumerate()
            .map(|(i, (ch_title, _))| {
                format!(
                    r#"<li><a href="chapter{}.xhtml">{}</a></li>"#,
                    i + 1,
                    escape_xml(ch_title)
                )
            })
            .collect::<Vec<_>>()
            .join("\n    ");
        let toc_xhtml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE html PUBLIC "-//W3C//DTD XHTML 1.1//EN" "http://www.w3.org/TR/xhtml11/DTD/xhtml11.dtd">
<html xmlns="http://www.w3.org/1999/xhtml">
<head>
  <title>Table of Contents</title>
</head>
<body>
  <h1>Table of Contents</h1>
  <ul>
    {toc_links}
  </ul>
</body>
</html>"#
        );
        zip.start_file("OEBPS/toc.xhtml", deflated).unwrap();
        zip.write_all(toc_xhtml.as_bytes()).unwrap();

        for (i, (ch_title, content)) in chapters.iter().enumerate() {
            let chapter_xhtml = format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE html PUBLIC "-//W3C//DTD XHTML 1.1//EN" "http://www.w3.org/TR/xhtml11/DTD/xhtml11.dtd">
<html xmlns="http://www.w3.org/1999/xhtml">
<head>
  <title>{title}</title>
</head>
<body>
  <h1>{title}</h1>
    {content}
</body>
</html>"#,
                title = escape_xml(ch_title),
            );
            zip.start_file(format!("OEBPS/chapter{}.xhtml", i + 1), deflated)
                .unwrap();
            zip.write_all(chapter_xhtml.as_bytes()).unwrap();
        }

        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn from_bytes_reads_language() {
        let bytes = build_epub("Lang Book", "A", Some("es"), &[("Ch", "<p>Hello.</p>")]);
        let book = EpubBook::from_bytes(bytes).unwrap();
        assert_eq!(book.language.as_deref(), Some("es"));
        assert_eq!(book.title, "A - Lang Book");
    }

    #[test]
    fn from_bytes_keeps_bcp47_language_verbatim() {
        let bytes = build_epub("Lang Book", "A", Some("en-US"), &[("Ch", "<p>Hello.</p>")]);
        assert_eq!(
            EpubBook::from_bytes(bytes).unwrap().language.as_deref(),
            Some("en-US")
        );
    }

    #[test]
    fn from_bytes_omits_language_when_dc_language_missing() {
        let bytes = build_epub("Lang Book", "A", None, &[("Ch", "<p>Hello.</p>")]);
        assert_eq!(EpubBook::from_bytes(bytes).unwrap().language, None);
    }

    #[test]
    fn from_bytes_rejects_garbage() {
        assert!(EpubBook::from_bytes(b"not an epub".to_vec()).is_err());
    }

    #[test]
    fn load_matches_from_bytes() {
        let bytes = build_epub("T", "C", Some("de"), &[("One", "<p>Hi.</p>")]);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.epub");
        std::fs::write(&path, &bytes).unwrap();
        let from_path = EpubBook::load(&path).unwrap();
        let from_mem = EpubBook::from_bytes(bytes).unwrap();
        assert_eq!(from_path.title, from_mem.title);
        assert_eq!(from_path.language, from_mem.language);
        assert_eq!(from_path.chapters.len(), from_mem.chapters.len());
    }

    fn el(html: &str) -> scraper::Html {
        Html::parse_fragment(html)
    }

    fn root_child(doc: &Html) -> ElementRef<'_> {
        // Fragment trees wrap under html without a selectable body.
        for child in doc.root_element().children() {
            if let Some(elem) = ElementRef::wrap(child) {
                if elem.value().name() != "html" {
                    return elem;
                }
            }
        }
        panic!("fragment root");
    }

    #[test]
    fn sanitizer_allows_b() {
        let doc = el("<b>bold</b>");
        assert_eq!(get_sanitized_html(root_child(&doc), true), "<b>bold</b>");
    }

    #[test]
    fn sanitizer_strips_span_to_text() {
        let doc = el("<span>forbidden</span>");
        assert_eq!(get_sanitized_html(root_child(&doc), true), "forbidden");
    }

    #[test]
    fn sanitizer_nests_allowed_tags() {
        let doc = el("<b>bold <i>italic</i></b>");
        assert_eq!(
            get_sanitized_html(root_child(&doc), true),
            "<b>bold <i>italic</i></b>"
        );
    }

    #[test]
    fn sanitizer_flattens_span_inside_b() {
        let doc = el("<b>bold <span>forbidden</span></b>");
        assert_eq!(
            get_sanitized_html(root_child(&doc), true),
            "<b>bold forbidden</b>"
        );
    }

    #[test]
    fn sanitizer_keeps_br() {
        let doc = el("<b>foo<br>bar</b>");
        assert_eq!(
            get_sanitized_html(root_child(&doc), true),
            "<b>foo<br>bar</b>"
        );
    }

    #[test]
    fn sanitizer_drops_bounding_tag() {
        let doc = el("<p>foo <br> bar</p>");
        assert_eq!(
            get_sanitized_html(root_child(&doc), false),
            "foo <br> bar"
        );
    }

    #[test]
    fn sanitizer_escapes_text_nodes() {
        let doc = el("<b>&lt;script&gt;alert(1)&lt;/script&gt;</b>");
        assert_eq!(
            get_sanitized_html(root_child(&doc), true),
            "<b>&lt;script&gt;alert(1)&lt;/script&gt;</b>"
        );
    }

    #[test]
    fn sanitizer_escapes_ampersand() {
        let doc = el("<b>Tom &amp; Jerry</b>");
        assert_eq!(
            get_sanitized_html(root_child(&doc), true),
            "<b>Tom &amp; Jerry</b>"
        );
    }

    #[test]
    fn from_bytes_extracts_italic_and_bold() {
        let bytes = build_epub(
            "Test Book",
            "Test Author",
            Some("en"),
            &[(
                "Chapter One",
                "<p>second paragraph with some <em>italic</em> and <b>bold</b> text.</p>",
            )],
        );
        let book = EpubBook::from_bytes(bytes).unwrap();
        let html: String = book
            .chapters
            .iter()
            .flat_map(|c| c.paragraphs.iter())
            .map(|p| p.html.as_str())
            .collect();
        assert!(html.contains("<em>italic</em>"), "{html}");
        assert!(html.contains("<b>bold</b>"), "{html}");
    }
}
