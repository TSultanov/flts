import { defineConfig, devices } from '@playwright/test';

// Real-backend tier: the frontend talks to a headless `app` binary over the WS
// bridge instead of the mocks. Helpers branch on `PLAYWRIGHT_REAL`
// (helpers/backend-mode.ts); set here so a bare `playwright test -c` run
// behaves like the pnpm script.
process.env.PLAYWRIGHT_REAL ||= 'true';

export default defineConfig({
  testDir: './tests/e2e',
  // Enabled here: tests/e2e/real/** plus app, text-import, epub-import,
  // chapters-panel, chapter-translate-all, chapter-translation-ratio —
  // everything that only needs the shared helper contract. Each ignore below
  // names what blocks it.
  testIgnore: [
    // Mock-only `window.__test` surfaces with no real-backend equivalent.
    'anki-sync.spec.ts', // __test.setAnkiSyncStatus / getSyncAnkiNowCalls
    'dialogs.spec.ts', // __test.seedBook
    'lyrics.spec.ts', // __mockSpotifyState / __mockLyrics / __mockPlatform
    'chapter-initial-translation-batch.spec.ts', // __test.getTranslationsBatchCalls
    // Seed fields the real pipeline cannot forge.
    'chapter-reading-state.spec.ts', // readingState
    'chapter-session-position.spec.ts', // readingState
    'chapter-summary-status.spec.ts', // summaryStatus + advanceSummaryGeneration
    'word-view-panel.spec.ts', // wordInfos
    'tap-to-reveal.spec.ts', // config seeding + per-word familiarity
    'anki-familiarity.spec.ts', // per-word familiarity + emitCardsUpdated
    // Segment text that deliberately diverges from the paragraph original;
    // real segments are sliced out of the original, so it cannot diverge.
    'paragraph-view.spec.ts', // + setTranslateConfig/inFlight in most cases
    'paragraph-view-multipage.spec.ts', // same, plus familiarity overlays
  ],
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: 0,
  workers: 4,
  reporter: [['list'], ['html', { open: 'never', outputFolder: 'playwright-report-real' }]],
  globalSetup: './tests/real/global-setup.ts',
  use: {
    baseURL: 'http://localhost:5181',
    trace: 'retain-on-failure',
  },
  projects: [{ name: 'real', use: { ...devices['Desktop Chrome'] } }],
  webServer: {
    command: 'PLAYWRIGHT_REAL=true pnpm dev --port 5181',
    url: 'http://localhost:5181',
    reuseExistingServer: !process.env.CI,
  },
});
