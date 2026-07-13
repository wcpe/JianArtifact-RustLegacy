// 业务契约 handlers（M1-FR-035）：覆盖 OpenAPI 契约全部 operation + devmock 自身探测端点。
// 谓词带 * 通配源站：Node 环境没有 document.baseURI，相对路径谓词不会被匹配。
import { http, HttpResponse } from 'msw';
import type { components } from './schema';
import { isScenarioId } from './scenarios';
import {
  createStore,
  switchScenario,
  type ApiError,
  type DevmockStore,
  type StoreResult,
} from './store';

type Schemas = components['schemas'];
type ErrorBody = Schemas['ErrorBody'];
type LoginRequest = Schemas['LoginRequest'];
type ChangePasswordRequest = Schemas['ChangePasswordRequest'];
type CreateTokenRequest = Schemas['CreateTokenRequest'];
type CreateUserRequest = Schemas['CreateUserRequest'];
type UpdateUserRequest = Schemas['UpdateUserRequest'];
type CreateRepoRequest = Schemas['CreateRepoRequest'];
type UpdateRepoRequest = Schemas['UpdateRepoRequest'];
type SetAclRequest = Schemas['SetAclRequest'];
type MigrationSource = Schemas['MigrationSource'];
type PlanRequest = Schemas['PlanRequest'];

/** 仓储错误 → 契约一致的错误响应体（ErrorBody：code/message）。 */
function errorOf(error: ApiError): Response {
  return HttpResponse.json<ErrorBody>(
    { code: error.code, message: error.message },
    { status: error.status },
  );
}

/** 仓储结果 → JSON 响应：成功用给定状态码，失败按错误语义返回。 */
function jsonOf<T>(result: StoreResult<T>, status = 200): Response {
  if (!result.ok) return errorOf(result.error);
  // 契约响应体均为纯数据对象/数组，这里对 msw 的 JSON 载荷约束做一次安全收窄
  return HttpResponse.json(result.value as unknown as Record<string, unknown> | unknown[], {
    status,
  });
}

/** 仓储结果 → 无内容响应（成功 204）。 */
function emptyOf(result: StoreResult<null>): Response {
  return result.ok ? new HttpResponse(null, { status: 204 }) : errorOf(result.error);
}

/** 路径参数统一取为字符串。 */
function stringOf(value: string | readonly string[] | undefined): string {
  return typeof value === 'string' ? value : '';
}

/** 读取整数查询参数（缺失或非法时返回 undefined，交由数据模型按 NFR-16 取默认）。 */
function intParam(url: URL, name: string): number | undefined {
  const raw = url.searchParams.get(name);
  if (raw === null) return undefined;
  const value = Number.parseInt(raw, 10);
  return Number.isNaN(value) ? undefined : value;
}

/** 读取分页查询参数。 */
function pageQuery(request: Request): { page?: number; pageSize?: number } {
  const url = new URL(request.url);
  return { page: intParam(url, 'page'), pageSize: intParam(url, 'page_size') };
}

/** 构建覆盖契约全部 operation 的 handlers（store 经 getter 注入，支持场景切换）。 */
export function createHandlers(getStore: () => DevmockStore) {
  return [
    // —— devmock 自身探测与场景切换（mock 基础设施，不属业务契约）——
    http.get('*/__devmock/health', () => HttpResponse.json({ status: 'ok' })),
    http.get('*/__devmock/scenario', () => {
      return HttpResponse.json({ scenario: getStore().scenarioId });
    }),
    http.put('*/__devmock/scenario', async ({ request }) => {
      const body = (await request.json()) as { scenario?: string };
      if (!body.scenario || !isScenarioId(body.scenario)) {
        return HttpResponse.json<ErrorBody>(
          { code: 'INVALID_SCENARIO', message: '未知的场景标识' },
          { status: 400 },
        );
      }
      switchScenario(getStore(), body.scenario);
      return HttpResponse.json({ scenario: body.scenario });
    }),

    // —— health ——
    http.get('*/healthz', () => new HttpResponse(null, { status: 200 })),
    http.get('*/readyz', () =>
      getStore().isReady()
        ? new HttpResponse(null, { status: 200 })
        : new HttpResponse(null, { status: 503 }),
    ),

    // —— auth ——
    http.post('*/api/v1/auth/login', async ({ request }) => {
      const body = (await request.json()) as LoginRequest;
      return jsonOf(getStore().login(body.username, body.password));
    }),
    http.post('*/api/v1/auth/logout', () => emptyOf(getStore().logout())),
    http.post('*/api/v1/auth/refresh', () => jsonOf(getStore().refresh())),
    http.post('*/api/v1/auth/password', async ({ request }) => {
      const body = (await request.json()) as ChangePasswordRequest;
      return emptyOf(getStore().changePassword(body.old_password, body.new_password));
    }),

    // —— tokens ——
    http.get('*/api/v1/tokens', ({ request }) => {
      const url = new URL(request.url);
      const { page, pageSize } = pageQuery(request);
      const q = url.searchParams.get('q') ?? undefined;
      return jsonOf(getStore().listTokens(page, pageSize, q));
    }),
    http.post('*/api/v1/tokens', async ({ request }) => {
      const body = (await request.json()) as CreateTokenRequest;
      return jsonOf(getStore().createToken(body.name), 201);
    }),
    http.delete('*/api/v1/tokens/:id', ({ params }) =>
      emptyOf(getStore().revokeToken(Number(stringOf(params.id)))),
    ),

    // —— users ——
    http.get('*/api/v1/users', ({ request }) => {
      const url = new URL(request.url);
      const { page, pageSize } = pageQuery(request);
      const q = url.searchParams.get('q') ?? undefined;
      return jsonOf(getStore().listUsers(page, pageSize, q));
    }),
    http.post('*/api/v1/users', async ({ request }) => {
      const body = (await request.json()) as CreateUserRequest;
      return jsonOf(getStore().createUser(body), 201);
    }),
    http.patch('*/api/v1/users/:id', async ({ params, request }) => {
      const body = (await request.json()) as UpdateUserRequest;
      return jsonOf(getStore().updateUser(Number(stringOf(params.id)), body));
    }),

    // —— repos ——
    http.get('*/api/v1/repos', ({ request }) => {
      const url = new URL(request.url);
      const { page, pageSize } = pageQuery(request);
      const q = url.searchParams.get('q') ?? undefined;
      return jsonOf(getStore().listRepos(page, pageSize, q));
    }),
    http.post('*/api/v1/repos', async ({ request }) => {
      const body = (await request.json()) as CreateRepoRequest;
      return jsonOf(getStore().createRepo(body), 201);
    }),
    http.get('*/api/v1/repos/:name', ({ params }) =>
      jsonOf(getStore().getRepo(stringOf(params.name))),
    ),
    http.patch('*/api/v1/repos/:name', async ({ params, request }) => {
      const body = (await request.json()) as UpdateRepoRequest;
      return jsonOf(getStore().updateRepo(stringOf(params.name), body));
    }),
    http.delete('*/api/v1/repos/:name', ({ params, request }) => {
      const url = new URL(request.url);
      const mode = url.searchParams.get('mode');
      const confirm = request.headers.get('X-Confirm-Repository');
      return jsonOf(getStore().deleteRepo(stringOf(params.name), mode, confirm));
    }),
    http.get('*/api/v1/repos/:name/acl', ({ params }) =>
      jsonOf(getStore().aclList(stringOf(params.name))),
    ),
    http.put('*/api/v1/repos/:name/acl/:user_id', async ({ params, request }) => {
      const body = (await request.json()) as SetAclRequest;
      const userId = Number(stringOf(params.user_id));
      return jsonOf(getStore().aclSet(stringOf(params.name), userId, body.permission));
    }),
    http.delete('*/api/v1/repos/:name/acl/:user_id', ({ params }) =>
      emptyOf(getStore().aclRemove(stringOf(params.name), Number(stringOf(params.user_id)))),
    ),
    http.get('*/api/v1/repos/:name/files', ({ params, request }) => {
      const url = new URL(request.url);
      const { page, pageSize } = pageQuery(request);
      const path = url.searchParams.get('path') ?? '';
      return jsonOf(getStore().listFiles(stringOf(params.name), path, page, pageSize));
    }),
    http.get('*/api/v1/repos/:name/search', ({ params, request }) => {
      const url = new URL(request.url);
      const { page, pageSize } = pageQuery(request);
      const q = url.searchParams.get('q') ?? '';
      return jsonOf(getStore().search(stringOf(params.name), q, page, pageSize));
    }),
    http.get('*/api/v1/repos/:name/usage', ({ params }) =>
      jsonOf(getStore().usage(stringOf(params.name))),
    ),

    // —— 格式协议端点（独立于管理 API 契约，见 docs/API.md §2.1）：仅供开发预览下载 ——
    http.get('*/repository/:name/*', ({ params, request }) => {
      const name = stringOf(params.name);
      const pathname = new URL(request.url).pathname;
      const prefix = `/repository/${name}/`;
      const path = decodeURIComponent(pathname.slice(pathname.indexOf(prefix) + prefix.length));
      const result = getStore().downloadFile(name, path);
      if (!result.ok) return errorOf(result.error);
      return new HttpResponse(result.value, {
        status: 200,
        headers: { 'Content-Type': 'application/octet-stream' },
      });
    }),

    // —— migration ——
    http.post('*/api/v1/migration/discover', async ({ request }) => {
      const body = (await request.json()) as MigrationSource;
      return jsonOf(getStore().discover(body));
    }),
    http.post('*/api/v1/migration/plan', async ({ request }) => {
      const body = (await request.json()) as PlanRequest;
      return jsonOf(getStore().plan(body));
    }),
    http.get('*/api/v1/migration/tasks', ({ request }) => {
      const { page, pageSize } = pageQuery(request);
      return jsonOf(getStore().listTasks(page, pageSize));
    }),
    http.post('*/api/v1/migration/tasks', ({ request }) => {
      const url = new URL(request.url);
      return jsonOf(getStore().createTask(url.searchParams.get('plan_id') ?? ''), 202);
    }),
    http.get('*/api/v1/migration/tasks/:id', ({ params }) =>
      jsonOf(getStore().getTask(stringOf(params.id))),
    ),
    http.post('*/api/v1/migration/tasks/:id/pause', ({ params }) =>
      jsonOf(getStore().controlTask(stringOf(params.id), 'pause'), 202),
    ),
    http.post('*/api/v1/migration/tasks/:id/resume', ({ params }) =>
      jsonOf(getStore().controlTask(stringOf(params.id), 'resume'), 202),
    ),
    http.post('*/api/v1/migration/tasks/:id/cancel', ({ params }) =>
      jsonOf(getStore().controlTask(stringOf(params.id), 'cancel'), 202),
    ),
    http.post('*/api/v1/migration/tasks/:id/retry', ({ params }) =>
      jsonOf(getStore().controlTask(stringOf(params.id), 'retry'), 202),
    ),
    http.get('*/api/v1/migration/tasks/:id/report', ({ params }) =>
      jsonOf(getStore().getReport(stringOf(params.id))),
    ),
  ];
}

// 模块级默认 store：default 场景；node.ts / browser.ts 沿用 handlers 导出不受影响
const moduleStore = createStore('default');

/** 默认 handlers：绑定模块级 default 场景 store（可经场景端点切换）。 */
export const handlers = createHandlers(() => moduleStore);
