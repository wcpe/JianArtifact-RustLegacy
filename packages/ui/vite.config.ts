/// <reference types="vitest/config" />
// 共享组件库的构建与测试配置：库模式仅产出 ES 产物，测试运行在 jsdom 环境。
import { join } from 'node:path';
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

export default defineConfig({
  plugins: [react()],
  resolve: {
    // 路径别名 "@" 指向 src 目录，与 tsconfig 中的 paths 保持一致
    alias: {
      '@': join(import.meta.dirname, 'src'),
    },
  },
  build: {
    // 库模式构建：入口为统一出口文件，运行时依赖一律外部化、不打进产物
    lib: {
      entry: 'src/index.ts',
      formats: ['es'],
      fileName: 'index',
    },
    rollupOptions: {
      external: [/^react/, 'radix-ui', 'class-variance-authority', 'clsx', 'tailwind-merge'],
    },
  },
  test: {
    // 组件测试运行在 jsdom 模拟浏览器环境
    environment: 'jsdom',
    setupFiles: './src/test/setup.ts',
  },
});
