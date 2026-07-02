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
   * 超时通过 `Promise.race` 实现而不污染 fetch signal，避免在 MSW（mock Service Worker 1.x）
   * 这类仅校验 RequestInit.signal 类型的测试环境下抛"Expected signal to be AbortSignal"。
   * 真正取消进行中的请求需要外部传入 `AbortSignal`（见 `TestProxy` 用法）。
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
 * 注意：内部用 `Promise.race` 包超时 / 外部信号，不直接传 fetch 的 `signal`，
 * 避免在 MSW（mock Service Worker 1.x）等会对 RequestInit.signal 做精确类型校验的环境炸错。
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

  // 真实请求：fetch 不带 signal（避免 MSW/老 fetch 实现的类型校验）。
  // 超时与外部 abort 都用 Promise.race 兜底。
  const fetchPromise = (async (): Promise<Response> => {
    try {
      return await fetch(`${API_BASE}${path}${buildQuery(query)}`, {
        method,
        headers,
        body: payload,
      });
    } catch (err) {
      throw new ApiError(0, 'network_error', err instanceof Error ? err.message : '网络错误');
    }
  })();

  const effectiveTimeout = timeoutMs ?? DEFAULT_TIMEOUT_MS;
  let timeoutTimer: ReturnType<typeof setTimeout> | null = null;
  const cleanup = () => {
    if (timeoutTimer !== null) {
      clearTimeout(timeoutTimer);
      timeoutTimer = null;
    }
  };

  // 三路竞速：fetch / 超时 / 外部 abort，任一达成即 resolve / reject 并清理监听。
  let response: Response;
  try {
    response = await new Promise<Response>((resolve, reject) => {
      let settled = false;
      const settle = (err: unknown) => {
        if (settled) return;
        settled = true;
        cleanup();
        if (externalSignal) {
          externalSignal.removeEventListener('abort', onExternalAbort);
        }
        reject(err);
      };
      const onExternalAbort = () => {
        settle(
          new ApiError(
            0,
            'aborted',
            externalSignal?.reason instanceof Error ? externalSignal.reason.message : '已取消',
          ),
        );
      };
      if (externalSignal?.aborted) {
        settle(new ApiError(0, 'aborted', '已取消'));
        return;
      }
      if (externalSignal) {
        externalSignal.addEventListener('abort', onExternalAbort, { once: true });
      }
      timeoutTimer = setTimeout(() => {
        settle(new ApiError(408, 'timeout', '请求超时'));
      }, effectiveTimeout);
      fetchPromise.then(
        (v) => {
          if (settled) return;
          settled = true;
          cleanup();
          if (externalSignal) {
            externalSignal.removeEventListener('abort', onExternalAbort);
          }
          resolve(v);
        },
        (e) => settle(e),
      );
    });
  } catch (err) {
    cleanup();
    throw err;
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
