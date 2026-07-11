/// <reference types="vitest/config" />
// 组件验收站构建与测试配置：React 插件 + Tailwind v4 插件，测试运行在 jsdom 环境。
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';

export default defineConfig({
  plugins: [react(), tailwindcss()],
  test: {
    // 组件测试运行在 jsdom 模拟浏览器环境
    environment: 'jsdom',
    setupFiles: './src/test/setup.ts',
  },
});
