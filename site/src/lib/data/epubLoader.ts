import Epub from 'epubts';
import type { ContentElement } from 'epubts/dist/types';

const allowListedTags = ["em", "i", "b", "br"];

export interface EpubBook {
    title: string,
    chapters: EpubChapter[],
}

export interface EpubChapter {
    title: string,
    paragraphs: Paragraph[],
}

export interface Paragraph {
    text: string,
    html: string,
}

export async function parseEpub(file: File): Promise<EpubBook> {
    const epub = await Epub.load(file);

    const chapters: EpubChapter[] = [];
    for (const c of epub.spine.contents) {
        const tocElements = epub.toc.filter(t => {
            const tHrefDocument = t.href.split("#")[0].replace("OEBPS/", "");
            const cHrefDocument = c.href.replace("OEBPS/", "");
            return tHrefDocument === cHrefDocument;
        });
        const data = await epub.getRawChapter(c.id);
        chapters.push(...parseChapter(data, tocElements));
    }

    const title = [];
    if (epub.metadata.creator) {
        title.push(epub.metadata.creator);
    }
    if (epub.metadata.title) {
        title.push(epub.metadata.title);
    }
    return {
        title: title.join(' - '),
        chapters,
    };
}

function parseChapter(chapter: string, toc: ContentElement[]): EpubChapter[] {
    const parser = new DOMParser();
    const document = parser.parseFromString(chapter, "application/xhtml+xml");

    if (toc.length === 0) {
        return [
            {
                title: document.title,
                paragraphs: textBetweenAnchors(document, "", null),
            }
        ];
    }

    const ret: EpubChapter[] = [];

    for (let i = 0; i < toc.length; i++) {
        const tCurr = toc[i];
        const tNext = i + 1 < toc.length ? toc[i+1] : null;

        const startAnchor = splitAnchor(tCurr.href);
        const endAnchor = tNext ? splitAnchor(tNext.href) : null;

        const text = textBetweenAnchors(document, startAnchor, endAnchor);

        ret.push({
            title: tCurr.title,
            paragraphs: text
        })
    }

    return ret;
} 

function splitAnchor(href: string): string {
    return href.split('#')[1] ?? ""
}

function textBetweenAnchors(document: Document, anchor1: string, anchor2: string | null): Paragraph[] {
    const startElement = (anchor1 !== "" ? document.getElementById(anchor1) : document.body) ?? document.body;
    const endElement = anchor2 ? document.getElementById(anchor2) : null;

    return textBetween(startElement, endElement);
}

const defaultDisplayMemo = new Map<string, string>();
function getElementDefaultDisplay(tag: string) {
    if (defaultDisplayMemo.has(tag)) {
        return defaultDisplayMemo.get(tag);
    }

    let cStyle;
    let t = document.createElement(tag);

    document.body.appendChild(t);
    cStyle = window.getComputedStyle(t, "").display; 
    document.body.removeChild(t);

    defaultDisplayMemo.set(tag, cStyle);

    return cStyle;
}

function allChildrenAreInline(element: Element) {
    for (const c of element.children) {
        if (getElementDefaultDisplay(c.tagName) !== "inline") {
            return false;
        }
    }

    return true;
}

function textBetween(start: Element, end: Element | null): Paragraph[] {
    const texts: Paragraph[] = [];
    let current: Node | null = start;

    while (current && current !== end) {
        if (current.nodeType === Node.ELEMENT_NODE) {
            const element = current as Element;
            if ((!element.hasChildNodes() || allChildrenAreInline(element)) && element.textContent && element.textContent.trim() !== "") {
                texts.push({
                    text: element.textContent.trim(),
                    html: getSanitizedHtml(element, false).trim()
                });
            }
        }
        // Traverse into children first, then siblings, then up to parent
        if (current.firstChild && (current.nodeType === Node.ELEMENT_NODE && !allChildrenAreInline(current as Element))) {
            current = current.firstChild;
        } else {
            while (current && !current.nextSibling && current !== end) {
                current = current.parentNode;
            }
            if (current) current = current.nextSibling;
        }
    }
    return texts;
}

export function getSanitizedHtml(element: Element, keepBoundingTag = true) {
    if (keepBoundingTag && !allowListedTags.includes(element.tagName.toLowerCase())) {
        return element.textContent ?? "";
    }

    if (element.tagName.toLowerCase() === "br") {
        return "<br>";
    }

    let html = keepBoundingTag ? `<${element.tagName.toLowerCase()}>` : "";

    for (const child of Array.from(element.childNodes)) {
        if (child.nodeType === Node.ELEMENT_NODE) {
            html += getSanitizedHtml(child as Element);
        } else if (child.nodeType === Node.TEXT_NODE) {
            html += (child as Text).textContent;
        }
    }

    html += keepBoundingTag ? `</${element.tagName.toLowerCase()}>` : "";
    return html;
}