// 管理 API 端点封装：按域聚合类型化调用，页面只调这里、不散拼 URL。
// 响应/请求类型一律取自契约（components['schemas']），随契约变更同步红。
import type { components } from '@jianartifact/devmock/schema';
import { apiRequest } from './client';

type Schemas = components['schemas'];

// 分页查询参数（用 type 别名而非 interface：interface 不满足 Record 索引签名）
export type PageQuery = {
  page?: number;
  page_size?: number;
  // 名称子串筛选（可选）：缺省或空串表示不筛选，由后端做子串匹配
  q?: string;
};

// 首启初始化
export const setupApi = {
  status: () => apiRequest<Schemas['SetupStatus']>('/api/v1/setup/status'),
  create: (body: Schemas['SetupRequest']) =>
    apiRequest<undefined>('/api/v1/setup', { method: 'POST', body }),
};

// 认证
export const authApi = {
  login: (body: Schemas['LoginRequest']) =>
    apiRequest<Schemas['SessionToken']>('/api/v1/auth/login', { method: 'POST', body }),
  me: () => apiRequest<Schemas['UserInfo']>('/api/v1/auth/me'),
  refresh: () => apiRequest<Schemas['SessionToken']>('/api/v1/auth/refresh', { method: 'POST' }),
  // 204 无内容响应：返回值实际为 undefined
  logout: () => apiRequest<undefined>('/api/v1/auth/logout', { method: 'POST' }),
  // 修改本人口令（M1-FR-045）：旧口令校验通过后生效，204 无内容
  changePassword: (body: Schemas['ChangePasswordRequest']) =>
    apiRequest<undefined>('/api/v1/auth/password', { method: 'POST', body }),
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
  // 删除仓库（M1-FR-009）：显式模式（仅删配置/删除数据）+ 确认头原样携带仓库名防误触
  remove: (name: string, mode: 'config' | 'data', confirmName: string) =>
    apiRequest<Schemas['DeleteRepoResponse']>(`/api/v1/repos/${name}`, {
      method: 'DELETE',
      query: { mode },
      headers: { 'X-Confirm-Repository': confirmName },
    }),
  // 仓库 ACL（M1-FR-008）：路径以精确用户名为授权键，响应仍保留 user_id 与 username
  aclList: (name: string) => apiRequest<Schemas['AclEntry'][]>(`/api/v1/repos/${name}/acl`),
  aclSet: (name: string, username: string, permission: Schemas['Permission']) =>
    apiRequest<Schemas['AclEntry']>(`/api/v1/repos/${name}/acl/${encodeURIComponent(username)}`, {
      method: 'PUT',
      body: { permission },
    }),
  aclRemove: (name: string, username: string) =>
    apiRequest<undefined>(`/api/v1/repos/${name}/acl/${encodeURIComponent(username)}`, {
      method: 'DELETE',
    }),
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
  // 任务控制四动作（M1-FR-034）：非法状态转移由后端返回 409
  pause: (id: string) =>
    apiRequest<Schemas['TaskInfo']>(`/api/v1/migration/tasks/${id}/pause`, { method: 'POST' }),
  resume: (id: string) =>
    apiRequest<Schemas['TaskInfo']>(`/api/v1/migration/tasks/${id}/resume`, { method: 'POST' }),
  cancel: (id: string) =>
    apiRequest<Schemas['TaskInfo']>(`/api/v1/migration/tasks/${id}/cancel`, { method: 'POST' }),
  retry: (id: string, body?: Schemas['RetryTaskRequest']) =>
    apiRequest<Schemas['TaskInfo']>(`/api/v1/migration/tasks/${id}/retry`, {
      method: 'POST',
      body,
    }),
  finalize: (id: string, body?: Schemas['RetryTaskRequest']) =>
    apiRequest<Schemas['TaskInfo']>(`/api/v1/migration/tasks/${id}/finalize`, {
      method: 'POST',
      body,
    }),
};
