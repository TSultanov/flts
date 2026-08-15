import { describe, it, expect } from 'vitest';
import { suggestSourceLanguage } from './suggestSourceLanguage';

const productionIds = ['deu', 'eng', 'kat', 'rus', 'spa', 'zho'];
const mockE2eIds = ['deu', 'eng', 'fra', 'ita', 'jpn', 'kor', 'por', 'rus', 'spa', 'zho'];

describe('suggestSourceLanguage', () => {
    it('falls back when nothing was parsed', () => {
        expect(suggestSourceLanguage(null, productionIds)).toBe('eng');
        expect(suggestSourceLanguage(undefined, productionIds)).toBe('eng');
        expect(suggestSourceLanguage('', productionIds)).toBe('eng');
    });

    it('preselects a parsed id that is in the dropdown', () => {
        expect(suggestSourceLanguage('spa', productionIds)).toBe('spa');
        expect(suggestSourceLanguage('deu', productionIds)).toBe('deu');
    });

    it('falls back when the parsed id is not in the loaded dropdown', () => {
        expect(suggestSourceLanguage('fra', productionIds)).toBe('eng');
        expect(suggestSourceLanguage('nld', mockE2eIds)).toBe('eng');
        expect(suggestSourceLanguage('und', productionIds)).toBe('eng');
    });

    it('keeps the parsed id when the dropdown has not loaded yet', () => {
        expect(suggestSourceLanguage('spa', [])).toBe('spa');
    });

    it('uses the provided fallback', () => {
        expect(suggestSourceLanguage(null, productionIds, 'deu')).toBe('deu');
    });
});
