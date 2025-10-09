use std::path::Path;

use epub::doc::EpubDoc;
use scraper::{ElementRef, Html, Node, Selector};

const ALLOWED_TAGS: &[&str] = &["em", "i", "b", "br"];

pub struct EpubBook {
    pub title: String,
    pub chapters: Vec<EpubChapter>,
}

pub struct EpubChapter {
    pub title: String,
    pub paragraphs: Vec<EpubParagraph>,
}

pub struct EpubParagraph {
    pub text: String,
    pub html: String,
}

impl EpubBook {
    pub fn load(path: &Path) -> anyhow::Result<EpubBook> {
        let mut epub = EpubDoc::new(path)?;

        let mut chapters = Vec::new();

        // Clone spine to avoid borrow issues
        let spine_items = epub.spine.clone();
        let toc_items = epub.toc.clone();

        // Process spine contents
        for spine_item in &spine_items {
            // Get TOC elements that match this spine item
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
                    let (c_href, _) = epub.resources.get(&spine_item.idref).unwrap();
                    let c_href_doc = c_href.to_str().unwrap().replace("OEBPS/", "");
                    t_href_doc == c_href_doc
                })
                .collect();

            // Get chapter content
            if let Some((content, _)) = epub.get_resource_str(&spine_item.idref) {
                chapters.extend(parse_chapter(&content, &toc_elements)?);
            }
        }

        // Build title
        let mut title_parts = Vec::new();
        if let Some(creator) = epub.metadata.get("creator") {
            if !creator.is_empty() {
                title_parts.push(creator[0].clone());
            }
        }
        if let Some(title) = epub.metadata.get("title") {
            if !title.is_empty() {
                title_parts.push(title[0].clone());
            }
        }

        Ok(EpubBook {
            title: title_parts.join(" - "),
            chapters,
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
        if let Some(child_element) = ElementRef::wrap(child) {
            if !is_inline_element(child_element.value().name()) {
                return false;
            }
        }
    }
    true
}

fn is_inline_element(tag_name: &str) -> bool {
    // Common inline elements
    matches!(
        tag_name.to_lowercase().as_str(),
        "a" | "abbr"
            | "b"
            | "bdi"
            | "bdo"
            | "br"
            | "cite"
            | "code"
            | "data"
            | "dfn"
            | "em"
            | "i"
            | "kbd"
            | "mark"
            | "q"
            | "s"
            | "samp"
            | "small"
            | "span"
            | "strong"
            | "sub"
            | "sup"
            | "time"
            | "u"
            | "var"
    )
}

fn text_between(start: ElementRef, end: Option<ElementRef>) -> Vec<EpubParagraph> {
    let mut paragraphs = Vec::new();
    let mut current = Some(start);

    while let Some(elem) = current {
        // Check if we've reached the end
        if let Some(end_elem) = end {
            if elem.id() == end_elem.id() {
                break;
            }
        }

        // Check if this is a paragraph-like element
        let has_text = elem.text().any(|t| !t.trim().is_empty());
        if has_text && (elem.children().count() == 0 || all_children_are_inline(elem)) {
            let text = elem.text().collect::<String>().trim().to_string();
            if !text.is_empty() {
                let html = get_sanitized_html(elem, false).trim().to_string();
                paragraphs.push(EpubParagraph { text, html });
            }
        }

        // Traverse: children first, then siblings, then up to parent's sibling
        if !all_children_are_inline(elem) {
            if let Some(first_child) = elem.children().find_map(ElementRef::wrap) {
                current = Some(first_child);
                continue;
            }
        }

        // Try next sibling
        current = find_next_sibling(elem).or_else(|| {
            // Go up and find next sibling of parent
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

fn get_sanitized_html(element: ElementRef, keep_bounding_tag: bool) -> String {
    let tag_lower = element.value().name().to_lowercase();

    if keep_bounding_tag && !ALLOWED_TAGS.contains(&tag_lower.as_str()) {
        return element.text().collect::<String>();
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
                html.push_str(text);
            }
            _ => {}
        }
    }

    if keep_bounding_tag {
        html.push_str(&format!("</{}>", tag_lower));
    }

    html
}
