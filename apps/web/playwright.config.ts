// Playwright E2E 配置：真实浏览器驱动 vite dev server（开发模式自动启用 devmock，
// 每次页面加载 mock store 重建为 default 场景，用例间天然隔离）。
import { defineConfig, devices } from '@playwright/test';

export default defineConfig({
  testDir: './e2e',
  // 用例内已按需等待，整体超时给宽裕值防 CI 抖动
  timeout: 30_000,
  fullyParallel: false,
  retries: 0,
  reporter: [['list']],
  use: {
    baseURL: 'http://127.0.0.1:4173',
    // 容器内以 root 运行 chromium 需要关闭沙箱
    launchOptions: { args: ['--no-sandbox'] },
    trace: 'retain-on-failure',
  },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'] } }],
  webServer: {
    command: 'pnpm exec vite --port 4173 --strictPort --host 127.0.0.1',
    url: 'http://127.0.0.1:4173',
    reuseExistingServer: false,
    timeout: 60_000,
  },
});
