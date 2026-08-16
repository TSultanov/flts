import { defineConfig, devices } from '@playwright/test';

// Real-backend tier: the frontend talks to a headless `app` binary over the WS
// bridge instead of the mocks. The mock tier (playwright.config.ts) is untouched.
export default defineConfig({
  testDir: './tests/e2e',
  // Only tests/e2e/real/** belongs to this tier; Task 13 shrinks this list.
  testIgnore: /tests[/\\]e2e[/\\](?!real[/\\]).*\.spec\.ts$/,
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
