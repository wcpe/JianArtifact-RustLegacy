// API 客户端：统一处理 Bearer 鉴权头、错误结构解析、401 跳登录。
// 同源部署，所有管理 API 走相对路径 /api/v1，不硬编码后端地址。

/** localStorage 中存放访问令牌的键名。 */
const TOKEN_STORAGE_KEY = 'jianartifact.token';

/** 管理 API 基础前缀。 */
const API_BASE = '/api/v1';

/** 后端统一错误结构。 */
interface ApiErrorBody {
  error?: {
    code?: string;
    message?: string;
  };
}

/** 调用方可识别的 API 错误：携带 HTTP 状态码与后端错误码 / 文案。 */
export class ApiError extends Error {
  /** HTTP 状态码。 */
  readonly status: number;
  /** 后端稳定错误码（如 unauthorized / forbidden / conflict）。 */
  readonly code: string;

  constructor(status: number, code: string, message: string) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
    this.code = code;
  }
}

/** 401 未认证时的回调（由应用注入，用于清理会话并跳转登录）。 */
let onUnauthorized: (() => void) | null = null;

/** 注册 401 处理回调。 */
export function setUnauthorizedHandler(handler: () => void): void {
  onUnauthorized = handler;
}

/** 读取已保存的访问令牌；无则返回 null。 */
export function getToken(): string | null {
  return localStorage.getItem(TOKEN_STORAGE_KEY);
}

/** 保存访问令牌。 */
export function setToken(token: string): void {
  localStorage.setItem(TOKEN_STORAGE_KEY, token);
}

/** 清除访问令牌（登出 / 会话失效）。 */
export function clearToken(): void {
  localStorage.removeItem(TOKEN_STORAGE_KEY);
}

/** 请求选项。 */
interface RequestOptions {
  method?: string;
  body?: unknown;
  query?: Record<string, string | number | undefined>;
  /**
   * 请求超时（毫秒）。到达后抛 `ApiError(408, 'timeout', '请求超时')`。
   *
   * 留 `undefined` 表示走默认（30s，足够包住绝大多数管理 API；后端 `proxy_test` 等专用端点
   * 由调用方传较短值（如 10s）实现 UI 不卡死——见 `testProxy`）。
   *
   * 超时会中断传给 fetch 的真实 `AbortSignal`，避免底层请求继续占用连接。
   */
  timeoutMs?: number;
  /**
   * 外部 AbortSignal：调用方主动中断时立即抛 `ApiError(0, 'aborted', ...)`，常用于：
   * - 模态框关闭 / URL 改动时取消挂起的请求，避免写 setState-after-unmount；
   * - 撤销按钮等用户主动取消动作。
   *
   * 与 `timeoutMs` 互不冲突：超时与外部 abort 都能正常中断。
   */
  signal?: AbortSignal;
}

/** 默认请求超时：30s（覆盖绝大多数管理 API）。 */
const DEFAULT_TIMEOUT_MS = 30_000;

/** 拼接查询字符串（跳过 undefined 值）。 */
function buildQuery(query?: Record<string, string | number | undefined>): string {
  if (!query) return '';
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(query)) {
    if (value !== undefined && value !== '') {
      params.set(key, String(value));
    }
  }
  const qs = params.toString();
  return qs ? `?${qs}` : '';
}

/**
 * 发起一次管理 API 请求并解析响应。
 *
 * - 自动附加 Bearer 令牌（若已登录）。
 * - 解析后端 `{error:{code,message}}` 错误结构，抛出 ApiError。
 * - 401 时触发已注册的未认证回调（清理会话并跳登录）。
 * - 204 / 空响应体返回 undefined。
 * - 支持 `timeoutMs` 超时：到达后抛 `ApiError(408, 'timeout', '请求超时')`，UI 层据此判定超时。
 * - 支持外部 `AbortSignal`：外部 abort 时抛 `ApiError(0, 'aborted')`，供 UI 主动取消请求。
 *
 * 内部统一桥接到真实传给 fetch 的 `AbortSignal`，并保留外部取消与请求超时两种错误语义。
 */
export async function request<T>(path: string, options: RequestOptions = {}): Promise<T> {
  const { method = 'GET', body, query, timeoutMs, signal: externalSignal } = options;
  const headers: Record<string, string> = {};
  const token = getToken();
  if (token) {
    headers['Authorization'] = `Bearer ${token}`;
  }
  let payload: string | undefined;
  if (body !== undefined) {
    headers['Content-Type'] = 'application/json';
    payload = JSON.stringify(body);
  }

  if (externalSignal?.aborted) {
    throw new ApiError(0, 'aborted', '已取消');
  }

  const controller = new AbortController();
  const effectiveTimeout = timeoutMs ?? DEFAULT_TIMEOUT_MS;
  let abortKind: 'external' | 'timeout' | null = null;
  let rejectAbort: ((error: ApiError) => void) | null = null;
  const abortError = () => {
    if (abortKind === 'timeout') {
      return new ApiError(408, 'timeout', '请求超时');
    }
    const message =
      externalSignal?.reason instanceof Error ? externalSignal.reason.message : '已取消';
    return new ApiError(0, 'aborted', message);
  };
  const abortRequest = (kind: 'external' | 'timeout') => {
    if (abortKind) return;
    abortKind = kind;
    controller.abort();
    rejectAbort?.(abortError());
  };
  const abortPromise = new Promise<never>((_, reject) => {
    rejectAbort = reject;
  });
  const onExternalAbort = () => abortRequest('external');
  externalSignal?.addEventListener('abort', onExternalAbort, { once: true });

  const timeoutTimer = setTimeout(() => abortRequest('timeout'), effectiveTimeout);
  const requestInit: RequestInit = {
    method,
    headers,
    body: payload,
    signal: controller.signal,
  };
  const fetchPromise = (async () => {
    try {
      return await fetch(`${API_BASE}${path}${buildQuery(query)}`, requestInit);
    } catch (err) {
      // jsdom 与 Node fetch/MSW 可能来自不同 realm，后者会在真正发请求前拒绝前者的 signal。
      // 仅对此兼容性错误重试无 signal；真实浏览器始终走上面的可取消请求。
      const message = err instanceof Error ? err.message : '';
      if (message.includes('Expected signal') && message.includes('instance of AbortSignal')) {
        return fetch(`${API_BASE}${path}${buildQuery(query)}`, {
          method,
          headers,
          body: payload,
        });
      }
      throw err;
    }
  })().catch((err) => {
    if (abortKind || controller.signal.aborted) throw abortError();
    throw new ApiError(0, 'network_error', err instanceof Error ? err.message : '网络错误');
  });

  let response: Response;
  try {
    response = await Promise.race([fetchPromise, abortPromise]);
  } finally {
    clearTimeout(timeoutTimer);
    externalSignal?.removeEventListener('abort', onExternalAbort);
  }

  if (response.status === 401) {
    // 未认证 / 会话失效：触发回调清理并跳登录
    if (onUnauthorized) {
      onUnauthorized();
    }
  }

  if (!response.ok) {
    let code = 'error';
    let message = `请求失败（HTTP ${response.status}）`;
    try {
      const data = (await response.json()) as ApiErrorBody;
      if (data.error) {
        code = data.error.code ?? code;
        message = data.error.message ?? message;
      }
    } catch {
      // 响应体非 JSON：保留默认文案
    }
    throw new ApiError(response.status, code, message);
  }

  if (response.status === 204) {
    return undefined as T;
  }
  const text = await response.text();
  if (!text) {
    return undefined as T;
  }
  return JSON.parse(text) as T;
}
