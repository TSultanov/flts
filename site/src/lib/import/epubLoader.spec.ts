import { describe, it, expect } from 'vitest';
import { getSanitizedHtml, parseEpub } from './epubLoader';
import { createTestEpub } from '../../../tests/fixtures/epub-generator';

function createElementFromHtml(html: string): Element {
    const div = document.createElement('div');
    div.innerHTML = html;
    return div.firstElementChild!;
}

async function epubFile(language?: string | null): Promise<File> {
    const buf = await createTestEpub({
        title: 'Lang Book',
        chapters: [{ title: 'Ch', content: '<p>Hello.</p>' }],
        ...(language !== undefined ? { language } : {}),
    });
    return new File([new Uint8Array(buf)], 'book.epub', {
        type: 'application/epub+zip',
    });
}

describe('parseEpub language metadata', () => {
    it('returns dc:language when present', async () => {
        const book = await parseEpub(await epubFile('es'));
        expect(book.language).toBe('es');
    });

    it('returns a BCP-47 tag unchanged (parser lives in isolang)', async () => {
        const book = await parseEpub(await epubFile('en-US'));
        expect(book.language).toBe('en-US');
    });

    it('omits language when dc:language is missing', async () => {
        const book = await parseEpub(await epubFile(null));
        expect(book.language).toBeUndefined();
    });
});

describe('getSanitizedHtml', () => {
    it('allows allowed tags', () => {
        const el = createElementFromHtml('<b>bold</b>');
        expect(getSanitizedHtml(el)).toBe('<b>bold</b>');
    });

    it('removes forbidden tags and returns textContent', () => {
        const el = createElementFromHtml('<span>forbidden</span>');
        expect(getSanitizedHtml(el)).toBe('forbidden');
    });

    it('handles nested allowed tags', () => {
        const el = createElementFromHtml('<b>bold <i>italic</i></b>');
        expect(getSanitizedHtml(el)).toBe('<b>bold <i>italic</i></b>');
    });

    it('flattens forbidden tags inside allowed tags', () => {
        const el = createElementFromHtml('<b>bold <span>forbidden</span></b>');
        expect(getSanitizedHtml(el)).toBe('<b>bold forbidden</b>');
    });

    it('handles text nodes', () => {
        const el = createElementFromHtml('<b>plain text</b>');
        expect(getSanitizedHtml(el)).toBe('<b>plain text</b>');
    });

    it('handles <br> as allowed self-closing tag', () => {
        const el = createElementFromHtml('<b>foo<br>bar</b>');
        expect(getSanitizedHtml(el)).toBe('<b>foo<br>bar</b>');
    });

    it('handles deeply nested forbidden tags', () => {
        const el = createElementFromHtml('<b>foo <span>bar <span>baz</span></span></b>');
        expect(getSanitizedHtml(el)).toBe('<b>foo bar baz</b>');
    });

    it('handles bounding element', () => {
        const el = createElementFromHtml('<p>foo <br> bar</p>');
        expect(getSanitizedHtml(el, false)).toBe('foo <br> bar');
    })

    it('escapes special characters in text nodes to prevent HTML injection', () => {
        const el = document.createElement('b');
        el.appendChild(document.createTextNode('<script>alert(1)</script>'));
        expect(getSanitizedHtml(el)).toBe('<b>&lt;script&gt;alert(1)&lt;/script&gt;</b>');
    });

    it('escapes special characters on the forbidden-tag path', () => {
        const el = document.createElement('span');
        el.appendChild(document.createTextNode('a < b & c'));
        expect(getSanitizedHtml(el)).toBe('a &lt; b &amp; c');
    });

    it('preserves original html entities inside gaps', () => {
        // innerHTML decodes "&amp;" to "&"; getSanitizedHtml must re-encode it.
        const el = createElementFromHtml('<b>Tom &amp; Jerry</b>');
        expect(getSanitizedHtml(el)).toBe('<b>Tom &amp; Jerry</b>');
    });
});
