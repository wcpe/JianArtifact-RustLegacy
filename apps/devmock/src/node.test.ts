// devmock 自身探测端点的 Node 端拦截测试
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { createDevmockServer } from './node';

const server = createDevmockServer();

// 测试开始前启动拦截，结束后关闭释放
beforeAll(() => server.listen());
afterAll(() => server.close());

describe('devmock Node 端', () => {
  it('拦截健康探测端点并返回 ok', async () => {
    const response = await fetch('http://127.0.0.1/__devmock/health');
    const body = (await response.json()) as { status: string };
    expect(body.status).toBe('ok');
  });
});
