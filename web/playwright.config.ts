import { defineConfig, devices } from '@playwright/test';

const baseURL = process.env.BASE_URL ?? 'http://localhost:5173';
const isCI = !!process.env.CI;
const workers = process.env.PLAYWRIGHT_WORKERS ? Number(process.env.PLAYWRIGHT_WORKERS) : isCI ? 1 : undefined;

/**
 * Playwright configuration for Chimera Dashboard E2E tests.
 * @see https://playwright.dev/docs/test-configuration
 */
export default defineConfig({
  testDir: './tests/e2e',
  
  /* Run tests in files in parallel */
  fullyParallel: true,
  
  /* Fail the build on CI if you accidentally left test.only in the source code */
  forbidOnly: isCI,
  
  /* Retry on CI only */
  retries: isCI ? 2 : 0,
  
  /* Worker count is configurable; CI defaults to 1 */
  workers,
  
  /* Reporter to use */
  reporter: [
    ['html', { outputFolder: 'playwright-report' }],
    ['list'],
  ],
  
  /* Shared settings for all the projects below */
  use: {
    /* Base URL to use in actions like `await page.goto('/')` */
    baseURL,

    /* Collect trace on retry on CI; retain traces for local failures */
    trace: isCI ? 'on-first-retry' : 'retain-on-failure',
    
    /* Screenshot on failure */
    screenshot: 'only-on-failure',
  },

  /* Configure projects for major browsers */
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

  /* Run your local dev server before starting the tests.
     Always start a fresh server by default so tests never run against a stale
     process; opt in to reuse with REUSE_EXISTING_SERVER=true. */
  webServer: {
    command: 'npm run dev',
    url: baseURL,
    reuseExistingServer: process.env.REUSE_EXISTING_SERVER === 'true',
    timeout: 120 * 1000,
  },
});
