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

        // Both must be omittable — they're optional, so a Config without them
        // is still a valid Config (legacy frontend builds against legacy
        // config files).
        const withoutAnki: Config = {
            translationProvider: 'google',
            model: 0,
        };
        // touch the variable so unused-binding linters don't complain
        void withoutAnki;
    });
});
