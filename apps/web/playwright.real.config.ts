// Playwright 真实后端通道：先经 setup 项目创建首名管理员，再运行既有真实后端用例。
import { defineConfig, devices } from '@playwright/test';

export default defineConfig({
  testDir: './e2e/real-backend',
  outputDir: './test-results/real-backend',
  timeout: 45_000,
  fullyParallel: false,
  workers: 1,
  retries: 0,
  reporter: [['list']],
  use: {
    baseURL: 'http://127.0.0.1:4174',
    launchOptions: { args: ['--no-sandbox'] },
    trace: 'retain-on-failure',
  },
  projects: [
    {
      name: 'setup-real',
      testMatch: '**/setup.spec.ts',
      use: { ...devices['Desktop Chrome'], trace: 'off' },
    },
    {
      name: 'chromium-real',
      testIgnore: '**/setup.spec.ts',
      dependencies: ['setup-real'],
      use: { ...devices['Desktop Chrome'] },
    },
  ],
  webServer: [
    {
      command: 'node e2e/real-backend/start-server.mjs',
      url: 'http://127.0.0.1:18080/readyz',
      reuseExistingServer: false,
      timeout: 180_000,
    },
    {
      command: 'pnpm exec vite --port 4174 --strictPort --host 127.0.0.1',
      url: 'http://127.0.0.1:4174',
      env: { VITE_MOCK: 'false' },
      reuseExistingServer: false,
      timeout: 60_000,
    },
  ],
});
