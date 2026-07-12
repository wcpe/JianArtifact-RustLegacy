// 场景切换端点测试：经 msw 拦截层驱动 /__devmock/scenario 读写与鉴权语义。
import { setupServer } from 'msw/node';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { createHandlers } from './handlers';
import { createStore } from './store';
import type { ScenarioId } from './scenarios';

// 用测试私有的 store 实例，避免影响模块级默认 store
const store = createStore('default');
const server = setupServer(...createHandlers(() => store));

beforeAll(() => server.listen());
afterAll(() => server.close());

/** 切换到指定场景。 */
async function switchTo(scenario: string): Promise<Response> {
  return fetch('http://127.0.0.1/__devmock/scenario', {
    method: 'PUT',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ scenario }),
  });
}

/** 读取当前场景标识。 */
async function currentScenario(): Promise<ScenarioId> {
  const response = await fetch('http://127.0.0.1/__devmock/scenario');
  const body = (await response.json()) as { scenario: ScenarioId };
  return body.scenario;
}

describe('场景切换端点', () => {
  it('PUT 切换后 GET 反映新场景且数据整体重建', async () => {
    expect(await currentScenario()).toBe('default');
    expect((await switchTo('empty')).status).toBe(200);
    expect(await currentScenario()).toBe('empty');
    const repos = await fetch('http://127.0.0.1/api/v1/repos');
    expect(((await repos.json()) as { total: number }).total).toBe(0);
  });

  it('未知场景标识返回 400', async () => {
    const response = await switchTo('不存在的场景');
    expect(response.status).toBe(400);
    const body = (await response.json()) as { code: string };
    expect(body.code).toBe('INVALID_SCENARIO');
  });

  it('未登录访问受保护端点返回 401 错误体', async () => {
    await switchTo('default');
    const logout = await fetch('http://127.0.0.1/api/v1/auth/logout', { method: 'POST' });
    expect(logout.status).toBe(204);
    const response = await fetch('http://127.0.0.1/api/v1/tokens');
    expect(response.status).toBe(401);
    const body = (await response.json()) as { code: string };
    expect(body.code).toBe('UNAUTHORIZED');
  });

  it('restricted 场景下管理端点返回 403 错误体', async () => {
    await switchTo('restricted');
    const response = await fetch('http://127.0.0.1/api/v1/users');
    expect(response.status).toBe(403);
    const body = (await response.json()) as { code: string };
    expect(body.code).toBe('FORBIDDEN');
  });

  it('upstream-failure 场景下就绪自检返回 503', async () => {
    await switchTo('upstream-failure');
    const response = await fetch('http://127.0.0.1/readyz');
    expect(response.status).toBe(503);
    await switchTo('default');
    expect((await fetch('http://127.0.0.1/readyz')).status).toBe(200);
  });
});
