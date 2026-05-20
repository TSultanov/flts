import { describe, it, expect } from 'vitest';
import {
    faSync,
    faCheck,
    faExclamationCircle,
} from '@fortawesome/free-solid-svg-icons';
import {
    iconForState,
    isVisible,
    isSpinning,
    isClickDisabled,
} from './ankiSyncHelpers';

describe('iconForState', () => {
    it('returns faSync for idle and syncing', () => {
        expect(iconForState('idle')).toBe(faSync);
        expect(iconForState('syncing')).toBe(faSync);
    });
    it('returns faCheck for ok', () => {
        expect(iconForState('ok')).toBe(faCheck);
    });
    it('returns faExclamationCircle for err', () => {
        expect(iconForState('err')).toBe(faExclamationCircle);
    });
    it('returns null for unreachable (signals hide-the-button)', () => {
        expect(iconForState('unreachable')).toBeNull();
    });
});

describe('isVisible', () => {
    it('hides only on unreachable', () => {
        expect(isVisible('idle')).toBe(true);
        expect(isVisible('syncing')).toBe(true);
        expect(isVisible('ok')).toBe(true);
        expect(isVisible('err')).toBe(true);
        expect(isVisible('unreachable')).toBe(false);
    });
});

describe('isSpinning', () => {
    it('only spins while syncing', () => {
        expect(isSpinning('syncing')).toBe(true);
        for (const s of ['idle', 'ok', 'err', 'unreachable'] as const) {
            expect(isSpinning(s)).toBe(false);
        }
    });
});

describe('isClickDisabled', () => {
    it('disables only while syncing', () => {
        expect(isClickDisabled('syncing')).toBe(true);
        for (const s of ['idle', 'ok', 'err', 'unreachable'] as const) {
            expect(isClickDisabled(s)).toBe(false);
        }
    });
});
