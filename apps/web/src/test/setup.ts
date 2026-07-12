// 测试环境初始化：注入 jest-dom 自定义断言 + 每个用例后清理挂载的 DOM。
// vitest 非 globals 模式下 testing-library 不会自动 cleanup，须显式注册，否则渲染跨用例累积。
import '@testing-library/jest-dom/vitest';
import { cleanup } from '@testing-library/react';
import { afterEach } from 'vitest';

afterEach(() => {
  cleanup();
});
