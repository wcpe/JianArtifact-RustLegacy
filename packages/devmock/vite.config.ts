/// <reference types="vitest/config" />
// devmock 构建与测试配置：演示页走默认构建，测试运行在 Node 环境。
import { defineConfig } from 'vite';

export default defineConfig({
  test: {
    // 拦截逻辑面向 Node 端，测试无需浏览器模拟
    environment: 'node',
  },
});
