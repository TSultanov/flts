import { describe, it, expect } from 'vitest';
import { getSanitizedHtml } from './epubLoader';

function createElementFromHtml(html: string): Element {
    const div = document.createElement('div');
    div.innerHTML = html;
    return div.firstElementChild!;
}

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
});
