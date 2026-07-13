// 测试环境初始化：jest-dom 断言 + DOM 清理 + MSW mock 后端生命周期。
// 页面测试经真实 fetch 打到 devmock handlers（node 端拦截），验证 client→endpoints→mock 全链路。
import '@testing-library/jest-dom/vitest';
// 从 node 子路径导入：包根 index 会连带加载 browser 的 setupWorker（浏览器专用，node 环境会崩）
import { createDevmockServer } from '@jianartifact/devmock/node';
import { cleanup } from '@testing-library/react';
import { afterAll, afterEach, beforeAll } from 'vitest';

// Radix Select（item-aligned 定位与指针捕获）依赖的 DOM API 在 jsdom 中缺失，补空实现；
// 逐一判空避免覆盖将来 jsdom 提供的真实实现
if (typeof Element.prototype.scrollIntoView !== 'function') {
  Element.prototype.scrollIntoView = () => {};
}
if (typeof Element.prototype.hasPointerCapture !== 'function') {
  Element.prototype.hasPointerCapture = () => false;
}
if (typeof Element.prototype.setPointerCapture !== 'function') {
  Element.prototype.setPointerCapture = () => {};
}
if (typeof Element.prototype.releasePointerCapture !== 'function') {
  Element.prototype.releasePointerCapture = () => {};
}

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
