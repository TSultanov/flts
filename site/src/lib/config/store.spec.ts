import { describe, it, expectTypeOf } from 'vitest';
import type { Config } from './store';

describe('Config type', () => {
    it('accepts ankiEndpoint and ankiApiKey as optional strings', () => {
        const withAnki: Config = {
            translationProvider: 'google',
            model: 0,
            ankiEndpoint: 'http://127.0.0.1:8765',
            ankiApiKey: 'secret',
        };
        expectTypeOf(withAnki.ankiEndpoint).toEqualTypeOf<string | undefined>();
        expectTypeOf(withAnki.ankiApiKey).toEqualTypeOf<string | undefined>();

        // Must stay omittable: config files written without them are valid.
        const withoutAnki: Config = {
            translationProvider: 'google',
            model: 0,
        };
        void withoutAnki;
    });

    it('accepts tapToRevealTranslations as an optional boolean', () => {
        const withFlag: Config = {
            translationProvider: 'google',
            model: 0,
            tapToRevealTranslations: true,
        };
        expectTypeOf(withFlag.tapToRevealTranslations).toEqualTypeOf<boolean | undefined>();

        const withoutFlag: Config = {
            translationProvider: 'google',
            model: 0,
        };
        void withoutFlag;
    });
});
