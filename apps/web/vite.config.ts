// 管理端构建与测试配置：React 插件 + Tailwind v4 插件，测试运行在 jsdom 环境。
// 从 vitest/config 导入即带来 defineConfig 的 test 字段类型增强，无需三斜线引用。
import { configDefaults, defineConfig } from 'vitest/config';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';

export default defineConfig({
  plugins: [react(), tailwindcss()],
  test: {
    // 组件测试运行在 jsdom 模拟浏览器环境
    environment: 'jsdom',
    setupFiles: './src/test/setup.ts',
    // e2e 由 Playwright 独立驱动真实浏览器，不纳入 vitest（其 *.spec 非 vitest 用例）
    exclude: [...configDefaults.exclude, 'e2e/**'],
  },
});
