import { defineConfig } from '@playwright/test';
export default defineConfig({
  testDir: '.', testMatch: '*.browser.mjs', workers: 1, retries: 0,
  timeout: 30000, reporter: [['list'], ['html', { open: 'never' }]],
  use: { baseURL: 'http://127.0.0.1:1421', browserName: 'chromium', trace: 'retain-on-failure', screenshot: 'only-on-failure' },
  webServer: { command: 'node ../node_modules/vite/bin/vite.js --config vite.config.mjs', url: 'http://127.0.0.1:1421', reuseExistingServer: false },
});
