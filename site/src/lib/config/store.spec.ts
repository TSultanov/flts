import { describe, it, expect, expectTypeOf, vi } from 'vitest';

// Value-importing `./store` constructs Resource stores; they need $state and
// a Tauri webview. Stub the dep so modelsForDropdown can be unit-tested.
vi.mock('../data/tauri.svelte', () => ({
    Resource: class Resource {},
}));
vi.mock('@tauri-apps/api/core', () => ({
    invoke: vi.fn(() => Promise.resolve(undefined)),
}));

import { modelsForDropdown, resolveModelSelection, type Config } from './store';

describe('Config type', () => {
    it('accepts ankiEndpoint and ankiApiKey as optional strings', () => {
        const withAnki: Config = {
            translationProvider: 'google',
            model: '',
            ankiEndpoint: 'http://127.0.0.1:8765',
            ankiApiKey: 'secret',
        };
        expectTypeOf(withAnki.ankiEndpoint).toEqualTypeOf<string | undefined>();
        expectTypeOf(withAnki.ankiApiKey).toEqualTypeOf<string | undefined>();

        // Must stay omittable: config files written without them are valid.
        const withoutAnki: Config = {
            translationProvider: 'google',
            model: '',
        };
        void withoutAnki;
    });

    it('accepts tapToRevealTranslations as an optional boolean', () => {
        const withFlag: Config = {
            translationProvider: 'google',
            model: '',
            tapToRevealTranslations: true,
        };
        expectTypeOf(withFlag.tapToRevealTranslations).toEqualTypeOf<boolean | undefined>();

        const withoutFlag: Config = {
            translationProvider: 'google',
            model: '',
        };
        void withoutFlag;
    });
});

describe('modelsForDropdown', () => {
    it('keeps a saved id missing from the catalog', () => {
        const models = [
            { id: 'models/gemini-3.7-flash', name: 'Gemini 3.7 Flash', provider: 'google' as const },
        ];
        const { list, orphan } = modelsForDropdown(models, 'google', 'models/gemini-2.5-flash');
        expect(orphan).toBe(true);
        expect(list[0].id).toBe('models/gemini-2.5-flash');
        expect(list.map(m => m.id)).toContain('models/gemini-3.7-flash');
    });

    it('does not treat empty selection as orphan', () => {
        const { list, orphan } = modelsForDropdown([], 'google', '');
        expect(orphan).toBe(false);
        expect(list).toEqual([]);
    });
});

describe('resolveModelSelection', () => {
    it('resets to default on provider change even if the selected id is missing from models', () => {
        // Catalog membership is not consulted: a Gemini id that is not in the
        // OpenAI list must still reset when the provider changes.
        expect(
            resolveModelSelection(
                'google',
                'openai',
                'models/gemini-2.5-flash',
                'gpt-5-mini',
            ),
        ).toBe('gpt-5-mini');
    });

    it('keeps a same-provider orphan id', () => {
        expect(
            resolveModelSelection(
                'google',
                'google',
                'models/gemini-2.5-flash',
                'models/gemini-3.7-flash',
            ),
        ).toBe('models/gemini-2.5-flash');
    });
});
