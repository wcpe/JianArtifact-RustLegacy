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

  it('setup 非法请求与真实后端一致返回结构化四百', async () => {
    const requests = [
      new Request('http://127.0.0.1/api/v1/setup', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: '{',
      }),
      new Request('http://127.0.0.1/api/v1/setup', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
      }),
      new Request('http://127.0.0.1/api/v1/setup', {
        method: 'POST',
        headers: { 'Content-Type': 'text/plain' },
        body: '{}',
      }),
      new Request('http://127.0.0.1/api/v1/setup', {
        method: 'POST',
        body: new TextEncoder().encode('{}'),
      }),
    ];

    for (const request of requests) {
      const response = await fetch(request);
      expect(response.status).toBe(400);
      await expect(response.json()).resolves.toEqual({
        code: 'SETUP_REQUEST_INVALID',
        message: '初始化请求格式无效',
      });
    }
  });
});
