// 管理 API 客户端：原生 fetch 薄封装，请求/响应类型复用 openapi-typescript 生成的契约类型。
// 鉴权拒绝一律以后端结果为准（前端不复制授权判定）；会话令牌置于 Authorization: Bearer。
import type { components } from '@jianartifact/devmock/schema';

// 契约的统一错误响应体
type ErrorBody = components['schemas']['ErrorBody'];

// API 调用失败时抛出的错误：携带 HTTP 状态与契约错误码，供页面按类别处理
export class ApiClientError extends Error {
  readonly status: number;
  readonly code: string;

  constructor(status: number, code: string, message: string) {
    super(message);
    this.name = 'ApiClientError';
    this.status = status;
    this.code = code;
  }
}

// 查询参数：键为字符串，值为字符串/数字/布尔或缺省
export type QueryParams = Record<string, string | number | boolean | undefined>;

// 请求选项：方法、可选 JSON 体、可选查询参数
interface RequestOptions {
  method?: 'GET' | 'POST' | 'PATCH' | 'PUT' | 'DELETE';
  body?: unknown;
  query?: QueryParams;
  headers?: Record<string, string>;
}

// 当前会话令牌（内存持有；持久化由 AuthContext 经 localStorage 处理）
let sessionToken: string | null = null;

// 设置/清除会话令牌
export function setSessionToken(token: string | null): void {
  sessionToken = token;
}

// 拼接查询串（跳过 undefined）
function buildQuery(query?: QueryParams): string {
  if (!query) return '';
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(query)) {
    if (value !== undefined) params.set(key, String(value));
  }
  const qs = params.toString();
  return qs ? `?${qs}` : '';
}

// 发起一次 API 请求；2xx 返回解析后的 JSON（204 返回 undefined），否则抛 ApiClientError
export async function apiRequest<T>(path: string, options: RequestOptions = {}): Promise<T> {
  const { method = 'GET', body, query, headers = {} } = options;
  const requestHeaders: Record<string, string> = { ...headers };
  if (body !== undefined) requestHeaders['Content-Type'] = 'application/json';
  if (sessionToken) requestHeaders['Authorization'] = `Bearer ${sessionToken}`;

  const response = await fetch(`${path}${buildQuery(query)}`, {
    method,
    headers: requestHeaders,
    body: body === undefined ? undefined : JSON.stringify(body),
  });

  if (response.status === 204) return undefined as T;

  const text = await response.text();
  const payload: unknown = text ? JSON.parse(text) : undefined;

  if (!response.ok) {
    const err = payload as ErrorBody | undefined;
    throw new ApiClientError(
      response.status,
      err?.code ?? 'UNKNOWN',
      err?.message ?? `请求失败（HTTP ${response.status}）`,
    );
  }
  return payload as T;
}
