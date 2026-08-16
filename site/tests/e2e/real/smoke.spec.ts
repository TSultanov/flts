import { test, expect } from '../../real/fixtures';

test('app boots headless and serves real config over the bridge', async ({
  page,
}) => {
  await page.goto('/');
  await page.waitForFunction(() => !!(window as any).__bridgeDebugInvoke);
  const cfg = await page.evaluate(() =>
    (window as any).__bridgeDebugInvoke('get_config'),
  );
  expect(cfg).toMatchObject({
    translationProvider: 'google',
    targetLanguageId: 'eng',
  });
});

test('lrclib request log is reachable', async ({ harness }) => {
  await harness.lrclib.seed([{ artist: 'A', title: 'T', plainLyrics: 'la' }]);
  const reqs = await harness.lrclib.requests();
  expect(Array.isArray(reqs)).toBe(true);
});
