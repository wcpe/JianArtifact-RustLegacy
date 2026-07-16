// Playwright devmock E2E 配置：默认场景与首启场景使用隔离的 Vite 服务。
import { defineConfig, devices } from '@playwright/test';

export default defineConfig({
  testDir: './e2e',
  testIgnore: '**/real-backend/**',
  outputDir: './test-results/devmock',
  timeout: 30_000,
  fullyParallel: false,
  retries: 0,
  reporter: [['list']],
  use: {
    baseURL: 'http://127.0.0.1:4173',
    launchOptions: { args: ['--no-sandbox'] },
    trace: 'retain-on-failure',
  },
  projects: [
    {
      name: 'chromium',
      testIgnore: ['**/real-backend/**', '**/setup.spec.ts'],
      use: { ...devices['Desktop Chrome'] },
    },
    {
      name: 'chromium-setup',
      testMatch: '**/setup.spec.ts',
      use: {
        ...devices['Desktop Chrome'],
        baseURL: 'http://127.0.0.1:4175',
        trace: 'off',
      },
    },
  ],
  webServer: [
    {
      command: 'pnpm exec vite --port 4173 --strictPort --host 127.0.0.1',
      url: 'http://127.0.0.1:4173',
      env: { VITE_MOCK: 'true' },
      reuseExistingServer: false,
      timeout: 60_000,
    },
    {
      command: 'pnpm exec vite --port 4175 --strictPort --host 127.0.0.1',
      url: 'http://127.0.0.1:4175',
      env: { VITE_MOCK: 'true', VITE_MOCK_SCENARIO: 'setup-required' },
      reuseExistingServer: false,
      timeout: 60_000,
    },
  ],
});
