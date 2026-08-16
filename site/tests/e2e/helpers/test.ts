// Tier-aware `test`: the real project needs the worker harness (app + sims +
// bridge-port injection), the mock project needs plain Playwright. Specs that
// run in both tiers import from here instead of '@playwright/test'.
import { test as baseTest, expect } from '@playwright/test';
import { test as realTest } from '../../real/fixtures';
import { isRealMode } from './backend-mode';

export const test = isRealMode() ? realTest : baseTest;
export { expect };
