import { defineConfig, devices } from '@playwright/test';

export default defineConfig({
  testDir: './tests/e2e',
  // tests/e2e/real/** belongs to the real-backend tier (playwright.real.config.ts).
  testIgnore: /tests[/\\]e2e[/\\]real[/\\]/,
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 2 : 0,
  workers: process.env.CI ? 1 : undefined,
  reporter: [['html', { open: 'never' }]],
  use: {
    baseURL: 'http://localhost:5180',
    trace: 'on-first-retry',
  },

  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] },
    },

    {
      name: 'firefox',
      use: { ...devices['Desktop Firefox'] },
    },

    {
      name: 'webkit',
      use: { ...devices['Desktop Safari'] },
    },
  ],

  // Port 5180 keeps Playwright's server off the developer's `pnpm dev` on 5173.
  webServer: {
    command: 'PLAYWRIGHT=true pnpm dev --port 5180',
    url: 'http://localhost:5180',
    reuseExistingServer: !process.env.CI,
  },
});
