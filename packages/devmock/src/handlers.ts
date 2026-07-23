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
type SetupRequest = Schemas['SetupRequest'];
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
type RetryTaskRequest = Schemas['RetryTaskRequest'];

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

function invalidSetupRequest(): Response {
  return HttpResponse.json<ErrorBody>(
    { code: 'SETUP_REQUEST_INVALID', message: '初始化请求格式无效' },
    { status: 400 },
  );
}

function isSetupRequest(value: unknown): value is SetupRequest {
  if (typeof value !== 'object' || value === null) return false;
  const body = value as Record<string, unknown>;
  return (
    typeof body.token === 'string' &&
    typeof body.username === 'string' &&
    typeof body.password === 'string'
  );
}

/** setup 与真实后端一致，只接受 JSON 内容类型和三个字符串字段。 */
async function parseSetupRequest(request: Request): Promise<SetupRequest | null> {
  const mediaType = request.headers.get('Content-Type')?.split(';', 1)[0]?.trim().toLowerCase();
  const jsonType =
    mediaType === 'application/json' ||
    (mediaType !== undefined &&
      mediaType.startsWith('application/') &&
      mediaType.endsWith('+json'));
  if (!jsonType) return null;
  try {
    const body: unknown = await request.json();
    return isSetupRequest(body) ? body : null;
  } catch {
    return null;
  }
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

/** 从请求头读取 Bearer 令牌；缺失返回 null，格式错误保留为空令牌供无效会话判定。 */
function bearerToken(request: Request): string | null {
  const authorization = request.headers.get('Authorization');
  if (authorization === null) return null;
  return /^Bearer\s+(.+)$/i.exec(authorization)?.[1] ?? '';
}

/** 在同一同步步骤内校验会话并调用仓储操作，避免请求间临时身份切换。 */
function authenticated(
  request: Request,
  getStore: () => DevmockStore,
  action: (store: DevmockStore) => Response,
): Response {
  const store = getStore();
  const auth = store.authenticate(bearerToken(request));
  if (!auth.ok) {
    store.clearRequestContext();
    return errorOf(auth.error);
  }
  try {
    return action(store);
  } finally {
    store.clearRequestContext();
  }
}

/** 格式协议读取允许无凭据匿名访问 public；一旦携带凭据就必须严格校验。 */
function optionallyAuthenticated(
  request: Request,
  getStore: () => DevmockStore,
  action: (store: DevmockStore) => Response,
): Response {
  const store = getStore();
  const token = bearerToken(request);
  if (token === null) {
    store.beginAnonymousRequest();
  } else {
    const auth = store.authenticate(token);
    if (!auth.ok) {
      store.clearRequestContext();
      return errorOf(auth.error);
    }
  }
  try {
    return action(store);
  } finally {
    store.clearRequestContext();
  }
}

/** 受保护 JSON 请求先验凭据，解析完成后再次校验并原子调用仓储操作。 */
async function authenticatedJson<T>(
  request: Request,
  getStore: () => DevmockStore,
  action: (store: DevmockStore, body: T) => Response,
): Promise<Response> {
  const token = bearerToken(request);
  const initialStore = getStore();
  const initial = initialStore.authenticate(token);
  initialStore.clearRequestContext();
  if (!initial.ok) return errorOf(initial.error);
  const body = (await request.json()) as T;
  const store = getStore();
  const current = store.authenticate(token);
  if (!current.ok) {
    store.clearRequestContext();
    return errorOf(current.error);
  }
  try {
    return action(store, body);
  } finally {
    store.clearRequestContext();
  }
}

/** 受保护可选 JSON 请求：无 Content-Type 视为无请求体，有 JSON 类型则严格解析。 */
async function authenticatedOptionalJson<T>(
  request: Request,
  getStore: () => DevmockStore,
  action: (store: DevmockStore, body: T | undefined) => Response,
): Promise<Response> {
  const token = bearerToken(request);
  const initialStore = getStore();
  const initial = initialStore.authenticate(token);
  initialStore.clearRequestContext();
  if (!initial.ok) return errorOf(initial.error);
  let body: T | undefined;
  const contentType = request.headers.get('Content-Type');
  if (contentType !== null) {
    const mediaType = contentType.split(';', 1)[0]?.trim().toLowerCase();
    const jsonType =
      mediaType === 'application/json' ||
      (mediaType?.startsWith('application/') === true && mediaType.endsWith('+json'));
    if (!jsonType) {
      return errorOf({
        status: 400,
        code: 'MIGRATION_INVALID_REQUEST',
        message: '迁移请求字段或 JSON 格式无效',
      });
    }
    try {
      body = (await request.json()) as T;
    } catch {
      return errorOf({
        status: 400,
        code: 'MIGRATION_INVALID_REQUEST',
        message: '迁移请求字段或 JSON 格式无效',
      });
    }
  }
  const store = getStore();
  const current = store.authenticate(token);
  if (!current.ok) {
    store.clearRequestContext();
    return errorOf(current.error);
  }
  try {
    return action(store, body);
  } finally {
    store.clearRequestContext();
  }
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

    // —— setup ——
    http.get('*/api/v1/setup/status', () => {
      const result = getStore().setupStatus();
      if (!result.ok) return errorOf(result.error);
      return HttpResponse.json(result.value, { headers: { 'Cache-Control': 'no-store' } });
    }),
    http.post('*/api/v1/setup', async ({ request }) => {
      const body = await parseSetupRequest(request);
      if (body === null) return invalidSetupRequest();
      return emptyOf(getStore().setup(body.token, body.username, body.password));
    }),

    // —— auth ——
    http.post('*/api/v1/auth/login', async ({ request }) => {
      const body = (await request.json()) as LoginRequest;
      return jsonOf(getStore().login(body.username, body.password));
    }),
    http.get('*/api/v1/auth/me', ({ request }) =>
      authenticated(request, getStore, (store) => jsonOf(store.me())),
    ),
    http.post('*/api/v1/auth/logout', ({ request }) =>
      authenticated(request, getStore, (store) => emptyOf(store.logout())),
    ),
    http.post('*/api/v1/auth/refresh', ({ request }) =>
      authenticated(request, getStore, (store) => jsonOf(store.refresh())),
    ),
    http.post('*/api/v1/auth/password', ({ request }) =>
      authenticatedJson<ChangePasswordRequest>(request, getStore, (store, body) =>
        emptyOf(store.changePassword(body.old_password, body.new_password)),
      ),
    ),

    // —— tokens ——
    http.get('*/api/v1/tokens', ({ request }) => {
      const url = new URL(request.url);
      const { page, pageSize } = pageQuery(request);
      const q = url.searchParams.get('q') ?? undefined;
      return authenticated(request, getStore, (store) =>
        jsonOf(store.listTokens(page, pageSize, q)),
      );
    }),
    http.post('*/api/v1/tokens', ({ request }) =>
      authenticatedJson<CreateTokenRequest>(request, getStore, (store, body) =>
        jsonOf(store.createToken(body.name), 201),
      ),
    ),
    http.delete('*/api/v1/tokens/:id', ({ params, request }) =>
      authenticated(request, getStore, (store) =>
        emptyOf(store.revokeToken(Number(stringOf(params.id)))),
      ),
    ),

    // —— users ——
    http.get('*/api/v1/users', ({ request }) => {
      const url = new URL(request.url);
      const { page, pageSize } = pageQuery(request);
      const q = url.searchParams.get('q') ?? undefined;
      return authenticated(request, getStore, (store) =>
        jsonOf(store.listUsers(page, pageSize, q)),
      );
    }),
    http.post('*/api/v1/users', ({ request }) =>
      authenticatedJson<CreateUserRequest>(request, getStore, (store, body) =>
        jsonOf(store.createUser(body), 201),
      ),
    ),
    http.patch('*/api/v1/users/:id', ({ params, request }) =>
      authenticatedJson<UpdateUserRequest>(request, getStore, (store, body) =>
        jsonOf(store.updateUser(Number(stringOf(params.id)), body)),
      ),
    ),

    // —— repos ——
    http.get('*/api/v1/repos', ({ request }) => {
      const url = new URL(request.url);
      const { page, pageSize } = pageQuery(request);
      const q = url.searchParams.get('q') ?? undefined;
      return authenticated(request, getStore, (store) =>
        jsonOf(store.listRepos(page, pageSize, q)),
      );
    }),
    http.post('*/api/v1/repos', ({ request }) =>
      authenticatedJson<CreateRepoRequest>(request, getStore, (store, body) =>
        jsonOf(store.createRepo(body), 201),
      ),
    ),
    http.get('*/api/v1/repos/:name', ({ params, request }) =>
      authenticated(request, getStore, (store) => jsonOf(store.getRepo(stringOf(params.name)))),
    ),
    http.patch('*/api/v1/repos/:name', ({ params, request }) =>
      authenticatedJson<UpdateRepoRequest>(request, getStore, (store, body) =>
        jsonOf(store.updateRepo(stringOf(params.name), body)),
      ),
    ),
    http.delete('*/api/v1/repos/:name', ({ params, request }) => {
      const url = new URL(request.url);
      const mode = url.searchParams.get('mode');
      const confirm = request.headers.get('X-Confirm-Repository');
      return authenticated(request, getStore, (store) =>
        jsonOf(store.deleteRepo(stringOf(params.name), mode, confirm)),
      );
    }),
    http.get('*/api/v1/repos/:name/acl', ({ params, request }) =>
      authenticated(request, getStore, (store) => jsonOf(store.aclList(stringOf(params.name)))),
    ),
    http.put('*/api/v1/repos/:name/acl/:username', ({ params, request }) =>
      authenticatedJson<SetAclRequest>(request, getStore, (store, body) =>
        jsonOf(store.aclSet(stringOf(params.name), stringOf(params.username), body.permission)),
      ),
    ),
    http.delete('*/api/v1/repos/:name/acl/:username', ({ params, request }) =>
      authenticated(request, getStore, (store) =>
        emptyOf(store.aclRemove(stringOf(params.name), stringOf(params.username))),
      ),
    ),
    http.get('*/api/v1/repos/:name/files', ({ params, request }) => {
      const url = new URL(request.url);
      const { page, pageSize } = pageQuery(request);
      const path = url.searchParams.get('path') ?? '';
      return authenticated(request, getStore, (store) =>
        jsonOf(store.listFiles(stringOf(params.name), path, page, pageSize)),
      );
    }),
    http.get('*/api/v1/repos/:name/search', ({ params, request }) => {
      const url = new URL(request.url);
      const { page, pageSize } = pageQuery(request);
      const q = url.searchParams.get('q') ?? '';
      return authenticated(request, getStore, (store) =>
        jsonOf(store.search(stringOf(params.name), q, page, pageSize)),
      );
    }),
    http.get('*/api/v1/repos/:name/usage', ({ params, request }) =>
      authenticated(request, getStore, (store) => jsonOf(store.usage(stringOf(params.name)))),
    ),

    // —— 格式协议端点（独立于管理 API 契约，见 docs/API.md §2.1）：仅供开发预览下载 ——
    http.get('*/repository/:name/*', ({ params, request }) =>
      optionallyAuthenticated(request, getStore, (store) => {
        const name = stringOf(params.name);
        const pathname = new URL(request.url).pathname;
        const prefix = `/repository/${name}/`;
        const path = decodeURIComponent(pathname.slice(pathname.indexOf(prefix) + prefix.length));
        const result = store.downloadFile(name, path);
        if (!result.ok) return errorOf(result.error);
        return new HttpResponse(result.value, {
          status: 200,
          headers: { 'Content-Type': 'application/octet-stream' },
        });
      }),
    ),

    // —— migration ——
    http.post('*/api/v1/migration/discover', ({ request }) =>
      authenticatedJson<MigrationSource>(request, getStore, (store, body) =>
        jsonOf(store.discover(body)),
      ),
    ),
    http.post('*/api/v1/migration/plan', ({ request }) =>
      authenticatedJson<PlanRequest>(request, getStore, (store, body) => jsonOf(store.plan(body))),
    ),
    http.get('*/api/v1/migration/tasks', ({ request }) => {
      const { page, pageSize } = pageQuery(request);
      return authenticated(request, getStore, (store) => jsonOf(store.listTasks(page, pageSize)));
    }),
    http.post('*/api/v1/migration/tasks', ({ request }) => {
      const url = new URL(request.url);
      return authenticated(request, getStore, (store) =>
        jsonOf(store.createTask(url.searchParams.get('plan_id') ?? ''), 202),
      );
    }),
    http.get('*/api/v1/migration/tasks/:id', ({ params, request }) =>
      authenticated(request, getStore, (store) => jsonOf(store.getTask(stringOf(params.id)))),
    ),
    http.post('*/api/v1/migration/tasks/:id/pause', ({ params, request }) =>
      authenticated(request, getStore, (store) =>
        jsonOf(store.controlTask(stringOf(params.id), 'pause'), 202),
      ),
    ),
    http.post('*/api/v1/migration/tasks/:id/resume', ({ params, request }) =>
      authenticated(request, getStore, (store) =>
        jsonOf(store.controlTask(stringOf(params.id), 'resume'), 202),
      ),
    ),
    http.post('*/api/v1/migration/tasks/:id/cancel', ({ params, request }) =>
      authenticated(request, getStore, (store) =>
        jsonOf(store.controlTask(stringOf(params.id), 'cancel'), 202),
      ),
    ),
    http.post('*/api/v1/migration/tasks/:id/retry', ({ params, request }) =>
      authenticatedOptionalJson<RetryTaskRequest>(request, getStore, (store, body) =>
        jsonOf(store.controlTask(stringOf(params.id), 'retry', body), 202),
      ),
    ),
    http.post('*/api/v1/migration/tasks/:id/finalize', ({ params, request }) =>
      authenticatedOptionalJson<RetryTaskRequest>(request, getStore, (store, body) =>
        jsonOf(store.finalizeTask(stringOf(params.id), body), 202),
      ),
    ),
    http.get('*/api/v1/migration/tasks/:id/report', ({ params, request }) =>
      authenticated(request, getStore, (store) => jsonOf(store.getReport(stringOf(params.id)))),
    ),
  ];
}

// 模块级默认 store：default 场景；node.ts / browser.ts 沿用 handlers 导出不受影响
const moduleStore = createStore('default');

/** 默认 handlers：绑定模块级 default 场景 store（可经场景端点切换）。 */
export const handlers = createHandlers(() => moduleStore);
