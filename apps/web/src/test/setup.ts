// 测试环境初始化：jest-dom 断言 + DOM 清理 + MSW mock 后端生命周期。
// 页面测试经真实 fetch 打到 devmock handlers（node 端拦截），验证 client→endpoints→mock 全链路。
import '@testing-library/jest-dom/vitest';
// 从 node 子路径导入：包根 index 会连带加载 browser 的 setupWorker（浏览器专用，node 环境会崩）
import { createDevmockServer } from '@jianartifact/devmock/node';
import { cleanup } from '@testing-library/react';
import { afterAll, afterEach, beforeAll } from 'vitest';

const server = createDevmockServer();

beforeAll(() => {
  server.listen({ onUnhandledRequest: 'error' });
});

afterEach(() => {
  cleanup();
  server.resetHandlers();
});

afterAll(() => {
  server.close();
});
