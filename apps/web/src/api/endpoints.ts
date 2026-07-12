// 管理 API 端点封装：按域聚合类型化调用，页面只调这里、不散拼 URL。
// 响应/请求类型一律取自契约（components['schemas']），随契约变更同步红。
import type { components } from '@jianartifact/devmock/schema';
import { apiRequest } from './client';

type Schemas = components['schemas'];

// 分页查询参数（用 type 别名而非 interface：interface 不满足 Record 索引签名）
export type PageQuery = {
  page?: number;
  page_size?: number;
};

// 认证
export const authApi = {
  login: (body: Schemas['LoginRequest']) =>
    apiRequest<Schemas['SessionToken']>('/api/v1/auth/login', { method: 'POST', body }),
  refresh: () => apiRequest<Schemas['SessionToken']>('/api/v1/auth/refresh', { method: 'POST' }),
  // 204 无内容响应：返回值实际为 undefined
  logout: () => apiRequest<undefined>('/api/v1/auth/logout', { method: 'POST' }),
};

// API Token
export const tokenApi = {
  list: (query?: PageQuery) =>
    apiRequest<Schemas['TokenListResponse']>('/api/v1/tokens', { query }),
  create: (body: Schemas['CreateTokenRequest']) =>
    apiRequest<Schemas['CreatedToken']>('/api/v1/tokens', { method: 'POST', body }),
  revoke: (id: number) => apiRequest<undefined>(`/api/v1/tokens/${id}`, { method: 'DELETE' }),
};

// 用户管理
export const userApi = {
  list: (query?: PageQuery) => apiRequest<Schemas['UserListResponse']>('/api/v1/users', { query }),
  create: (body: Schemas['CreateUserRequest']) =>
    apiRequest<Schemas['UserInfo']>('/api/v1/users', { method: 'POST', body }),
  update: (id: number, body: Schemas['UpdateUserRequest']) =>
    apiRequest<Schemas['UserInfo']>(`/api/v1/users/${id}`, { method: 'PATCH', body }),
};

// 仓库
export const repoApi = {
  list: (query?: PageQuery) => apiRequest<Schemas['RepoListResponse']>('/api/v1/repos', { query }),
  create: (body: Schemas['CreateRepoRequest']) =>
    apiRequest<Schemas['RepoInfo']>('/api/v1/repos', { method: 'POST', body }),
  detail: (name: string) => apiRequest<Schemas['RepoInfo']>(`/api/v1/repos/${name}`),
  update: (name: string, body: Schemas['UpdateRepoRequest']) =>
    apiRequest<Schemas['RepoInfo']>(`/api/v1/repos/${name}`, { method: 'PATCH', body }),
  files: (name: string, query?: PageQuery & { path?: string }) =>
    apiRequest<Schemas['FileListResponse']>(`/api/v1/repos/${name}/files`, { query }),
  search: (name: string, query: PageQuery & { q: string }) =>
    apiRequest<Schemas['FileListResponse']>(`/api/v1/repos/${name}/search`, { query }),
  usage: (name: string) => apiRequest<Schemas['UsageSnippet'][]>(`/api/v1/repos/${name}/usage`),
};

// 迁移
export const migrationApi = {
  discover: (body: Schemas['MigrationSource']) =>
    apiRequest<Schemas['DiscoveryResult']>('/api/v1/migration/discover', { method: 'POST', body }),
  plan: (body: Schemas['PlanRequest']) =>
    apiRequest<Schemas['MigrationPlan']>('/api/v1/migration/plan', { method: 'POST', body }),
  createTask: (planId: string) =>
    apiRequest<Schemas['TaskInfo']>('/api/v1/migration/tasks', {
      method: 'POST',
      query: { plan_id: planId },
    }),
  listTasks: (query?: PageQuery) =>
    apiRequest<Schemas['TaskListResponse']>('/api/v1/migration/tasks', { query }),
  taskDetail: (id: string) => apiRequest<Schemas['TaskInfo']>(`/api/v1/migration/tasks/${id}`),
  report: (id: string) =>
    apiRequest<Schemas['MigrationReport']>(`/api/v1/migration/tasks/${id}/report`),
};
