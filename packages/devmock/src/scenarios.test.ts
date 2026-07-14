// 场景切换端点测试：经 msw 拦截层驱动 /__devmock/scenario 读写与鉴权语义。
import { setupServer } from 'msw/node';
import { afterAll, beforeAll, beforeEach, describe, expect, it } from 'vitest';
import { createHandlers } from './handlers';
import { createStore } from './store';
import type { ScenarioId } from './scenarios';

// 用测试私有的 store 实例，避免影响模块级默认 store
const store = createStore('default');
const server = setupServer(...createHandlers(() => store));

beforeAll(() => server.listen());
beforeEach(() => store.resetTo('default'));
afterAll(() => server.close());

/** 使用测试凭据登录并返回会话令牌。 */
async function login(username = 'admin', password = 'admin123'): Promise<string> {
  const response = await fetch('http://127.0.0.1/api/v1/auth/login', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ username, password }),
  });
  const body = (await response.json()) as { token: string };
  return body.token;
}

/** 携带 Bearer 会话令牌发起请求。 */
function authorizedFetch(path: string, token: string, init: RequestInit = {}): Promise<Response> {
  const headers = new Headers(init.headers);
  headers.set('Authorization', `Bearer ${token}`);
  return fetch(`http://127.0.0.1${path}`, { ...init, headers });
}

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
    const token = await login();
    const repos = await authorizedFetch('/api/v1/repos', token);
    expect(((await repos.json()) as { total: number }).total).toBe(0);
  });

  it('未知场景标识返回 400', async () => {
    const response = await switchTo('不存在的场景');
    expect(response.status).toBe(400);
    const body = (await response.json()) as { code: string };
    expect(body.code).toBe('INVALID_SCENARIO');
  });

  it('auth/me 不接受场景预置会话，登录令牌才能读取当前身份', async () => {
    await switchTo('default');
    const anonymous = await fetch('http://127.0.0.1/api/v1/auth/me');
    expect(anonymous.status).toBe(401);
    expect((await anonymous.json()) as { code: string }).toMatchObject({
      code: 'AUTH_UNAUTHENTICATED',
    });

    const token = await login();
    const me = await authorizedFetch('/api/v1/auth/me', token);
    expect(await me.json()).toMatchObject({ username: 'admin', role: 'admin' });
  });

  it('受保护端点区分缺失凭据与伪造凭据', async () => {
    const missing = await fetch('http://127.0.0.1/api/v1/tokens');
    expect(missing.status).toBe(401);
    expect((await missing.json()) as { code: string }).toMatchObject({
      code: 'AUTH_UNAUTHENTICATED',
    });

    const forged = await authorizedFetch('/api/v1/tokens', 'jwt_mock_forged');
    expect(forged.status).toBe(401);
    expect((await forged.json()) as { code: string }).toMatchObject({
      code: 'AUTH_INVALID_SESSION',
    });

    for (const path of ['/api/v1/auth/logout', '/api/v1/auth/refresh']) {
      const response = await authorizedFetch(path, 'jwt_mock_forged', { method: 'POST' });
      expect(response.status).toBe(401);
      expect((await response.json()) as { code: string }).toMatchObject({
        code: 'AUTH_INVALID_SESSION',
      });
    }

    const password = await authorizedFetch('/api/v1/auth/password', 'jwt_mock_forged', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ old_password: 'admin123', new_password: 'new-pass' }),
    });
    expect(password.status).toBe(401);
    expect((await password.json()) as { code: string }).toMatchObject({
      code: 'AUTH_INVALID_SESSION',
    });
  });

  it('格式下载允许匿名 public，但 private 与伪造凭据不降级', async () => {
    const publicResponse = await fetch('http://127.0.0.1/repository/raw-hosted/docs/manual.pdf');
    expect(publicResponse.status).toBe(200);

    const privateAnonymous = await fetch(
      'http://127.0.0.1/repository/npm-internal/@acme/ui/-/ui-1.0.0.tgz',
    );
    expect(privateAnonymous.status).toBe(404);

    const forged = await authorizedFetch(
      '/repository/raw-hosted/docs/manual.pdf',
      'jwt_mock_forged',
    );
    expect(forged.status).toBe(401);
    expect((await forged.json()) as { code: string }).toMatchObject({
      code: 'AUTH_INVALID_SESSION',
    });

    const token = await login();
    const privateAdmin = await authorizedFetch(
      '/repository/npm-internal/@acme/ui/-/ui-1.0.0.tgz',
      token,
    );
    expect(privateAdmin.status).toBe(200);
  });

  it('refresh 换发后旧令牌失效，新令牌继续访问受保护端点', async () => {
    const oldToken = await login();
    const refresh = await authorizedFetch('/api/v1/auth/refresh', oldToken, { method: 'POST' });
    expect(refresh.status).toBe(200);
    const { token: newToken } = (await refresh.json()) as { token: string };
    expect(newToken).not.toBe(oldToken);

    const oldResponse = await authorizedFetch('/api/v1/auth/refresh', oldToken, { method: 'POST' });
    expect(oldResponse.status).toBe(401);
    expect((await oldResponse.json()) as { code: string }).toMatchObject({
      code: 'AUTH_INVALID_SESSION',
    });
    expect((await authorizedFetch('/api/v1/auth/me', newToken)).status).toBe(200);
  });

  it('logout 与修改口令后原令牌立即失效', async () => {
    const logoutToken = await login();
    expect(
      (await authorizedFetch('/api/v1/auth/logout', logoutToken, { method: 'POST' })).status,
    ).toBe(204);
    const afterLogout = await authorizedFetch('/api/v1/auth/me', logoutToken);
    expect(afterLogout.status).toBe(401);
    expect((await afterLogout.json()) as { code: string }).toMatchObject({
      code: 'AUTH_INVALID_SESSION',
    });

    const passwordToken = await login();
    const changed = await authorizedFetch('/api/v1/auth/password', passwordToken, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ old_password: 'admin123', new_password: 'new-pass' }),
    });
    expect(changed.status).toBe(204);
    const afterChange = await authorizedFetch('/api/v1/auth/me', passwordToken);
    expect(afterChange.status).toBe(401);
    expect((await afterChange.json()) as { code: string }).toMatchObject({
      code: 'AUTH_INVALID_SESSION',
    });
  });

  it('restricted 场景下管理端点返回 403 错误体', async () => {
    await switchTo('restricted');
    const token = await login('dev', 'dev123');
    const response = await authorizedFetch('/api/v1/users', token);
    expect(response.status).toBe(403);
    const body = (await response.json()) as { code: string };
    expect(body.code).toBe('AUTH_FORBIDDEN');
  });

  it('upstream-failure 场景下就绪自检返回 503', async () => {
    await switchTo('upstream-failure');
    const response = await fetch('http://127.0.0.1/readyz');
    expect(response.status).toBe(503);
    await switchTo('default');
    expect((await fetch('http://127.0.0.1/readyz')).status).toBe(200);
  });
});
