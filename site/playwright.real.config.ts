import { defineConfig, devices } from "@playwright/test";

// Real-backend tier: the frontend talks to a headless `app` binary over the WS
// bridge. Set here so a bare `playwright test -c` matches the pnpm script;
// helpers branch on it (helpers/backend-mode.ts).
process.env.PLAYWRIGHT_REAL ||= "true";

// FLTS_E2E_PROD=1 serves the built frontend instead of the dev server, on its
// own port so a running dev server is never mistaken for it.
const prod = !!process.env.FLTS_E2E_PROD;
const port = prod ? 5182 : 5181;

const APP_FRONTEND_PORT = 5183;

export default defineConfig({
  testDir: "./tests/e2e",
  // Each ignore names what blocks it against a real backend.
  testIgnore: [
    // Mock-only `window.__test` surfaces with no real-backend equivalent.
    "anki-sync.spec.ts", // __test.setAnkiSyncStatus / getSyncAnkiNowCalls
    "dialogs.spec.ts", // __test.seedBook
    "lyrics.spec.ts", // __mockSpotifyState / __mockLyrics / __mockPlatform
    "appearance.spec.ts", // __test.seedBook
    "config-scroll.spec.ts", // __test.getConfig
    "chapter-initial-translation-batch.spec.ts", // __test.getTranslationsBatchCalls
    // Seed fields the real pipeline cannot forge.
    "chapter-reading-state.spec.ts", // readingState
    "chapter-session-position.spec.ts", // readingState
    "chapter-summary-status.spec.ts", // summaryStatus + advanceSummaryGeneration
    "word-view-panel.spec.ts", // wordInfos
    "tap-to-reveal.spec.ts", // config seeding + per-word familiarity
    "anki-familiarity.spec.ts", // per-word familiarity + emitCardsUpdated
    // Segment text that deliberately diverges from the paragraph original;
    // real segments are sliced out of the original, so it cannot diverge.
    "paragraph-view.spec.ts", // + setTranslateConfig/inFlight in most cases
    "paragraph-view-multipage.spec.ts", // same, plus familiarity overlays
    "paragraph-marks.spec.ts", // seeds marks a real translation would derive
    "chapter-page-shift.spec.ts", // 220 seeded paragraphs paired html/segments
    "word-overlay-geometry.spec.ts", // per-word familiarity + translation
  ],
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: 0,
  workers: 4,
  reporter: [
    ["list"],
    ["html", { open: "never", outputFolder: "playwright-report-real" }],
  ],
  globalSetup: "./tests/real/global-setup.ts",
  use: {
    baseURL: `http://localhost:${port}`,
    trace: "retain-on-failure",
  },
  projects: [
    { name: "real", use: { ...devices["Desktop Chrome"] } },
    // The app's own engine family; only the timing spec is worth running twice.
    ...(process.env.FLTS_PERF
      ? [{ name: "real-webkit", use: { ...devices["Desktop Safari"] } }]
      : []),
  ],
  webServer: [
    {
      command: prod
        ? `PLAYWRIGHT_REAL=true pnpm exec vite build --outDir dist-e2e && PLAYWRIGHT_REAL=true pnpm exec vite preview --outDir dist-e2e --port ${port} --strictPort`
        : `PLAYWRIGHT_REAL=true pnpm dev --port ${port}`,
      url: `http://localhost:${port}`,
      timeout: 180_000,
      reuseExistingServer: !process.env.CI,
    },
    // FLTS_PERF_APP=1: the frontend as shipped, with Tauri's own IPC, for
    // the app's own webview to load (see chapter-open-perf.spec.ts).
    ...(process.env.FLTS_PERF_APP
      ? [
          {
            command: `env -u PLAYWRIGHT_REAL -u PLAYWRIGHT pnpm exec vite build --outDir dist-e2e-app && env -u PLAYWRIGHT_REAL -u PLAYWRIGHT pnpm exec vite preview --outDir dist-e2e-app --port ${APP_FRONTEND_PORT} --strictPort`,
            url: `http://localhost:${APP_FRONTEND_PORT}`,
            timeout: 180_000,
            reuseExistingServer: !process.env.CI,
          },
        ]
      : []),
  ],
});
