// 有状态内存数据模型（M1-FR-035）：devmock 业务语义的唯一持有者。
// 所有方法同步、纯内存；测试凭据只存在于本包，生产构建不引入本包运行时。
import type { components } from './schema';
import { seedFor, type ScenarioId } from './scenarios';

type Schemas = components['schemas'];
type UserInfo = Schemas['UserInfo'];
type CreateUserRequest = Schemas['CreateUserRequest'];
type UpdateUserRequest = Schemas['UpdateUserRequest'];
type UserListResponse = Schemas['UserListResponse'];
type TokenInfo = Schemas['TokenInfo'];
type CreatedToken = Schemas['CreatedToken'];
type TokenListResponse = Schemas['TokenListResponse'];
type SessionToken = Schemas['SessionToken'];
type RepoInfo = Schemas['RepoInfo'];
type CreateRepoRequest = Schemas['CreateRepoRequest'];
type UpdateRepoRequest = Schemas['UpdateRepoRequest'];
type RepoListResponse = Schemas['RepoListResponse'];
type DeleteRepoResponse = Schemas['DeleteRepoResponse'];
type AclEntry = Schemas['AclEntry'];
type Permission = Schemas['Permission'];
type FileEntry = Schemas['FileEntry'];
type FileListResponse = Schemas['FileListResponse'];
type UsageSnippet = Schemas['UsageSnippet'];
type MigrationSource = Schemas['MigrationSource'];
type DiscoveryResult = Schemas['DiscoveryResult'];
type DiscoveredRepo = Schemas['DiscoveredRepo'];
type PlanRequest = Schemas['PlanRequest'];
type PlanConflict = Schemas['PlanConflict'];
type MigrationPlan = Schemas['MigrationPlan'];
type TaskInfo = Schemas['TaskInfo'];
type TaskStatus = Schemas['TaskStatus'];
type TaskListResponse = Schemas['TaskListResponse'];
type MigrationReport = Schemas['MigrationReport'];

/** 种子用户：契约信息 + 仅本包使用的测试口令。 */
export interface SeedUser {
  info: UserInfo;
  password: string;
}

/** 种子 Token：契约元数据 + 归属用户（存储永不含明文）。 */
export interface SeedToken {
  info: TokenInfo;
  userId: number;
}

/** 种子文件条目：仓库内完整路径 + 元信息，目录结构由路径推导。 */
export interface SeedFile {
  path: string;
  size: number;
  sha256: string;
  updated_at: string;
}

/** 种子仓库：契约信息 + ACL + 文件条目。 */
export interface SeedRepo {
  info: RepoInfo;
  acl: AclEntry[];
  files: SeedFile[];
}

/** 种子任务：契约信息 + 可选预置报告（无预置时按任务状态即时合成）。 */
export interface SeedTask {
  info: TaskInfo;
  report: MigrationReport | null;
}

/** 场景种子数据：DevmockStore 的完整初始状态。 */
export interface SeedData {
  users: SeedUser[];
  tokens: SeedToken[];
  repos: SeedRepo[];
  tasks: SeedTask[];
  /** 当前会话归属用户（null = 未登录）。 */
  session: { userId: number } | null;
  /** 上游是否故障（proxy 浏览与在线发现失败，就绪自检报未就绪）。 */
  upstreamDown: boolean;
}

/** 仓储层错误语义：HTTP 状态码 + 稳定错误码 + 中文消息。 */
export interface ApiError {
  status: number;
  code: string;
  message: string;
}

/** 失败分支的统一形状（可赋值给任意 StoreResult<T>）。 */
export type StoreFailure = { ok: false; error: ApiError };

/** 仓储方法统一返回：成功携带载荷，失败携带错误语义。 */
export type StoreResult<T> = { ok: true; value: T } | StoreFailure;

/** 任务控制动作。 */
export type TaskAction = 'pause' | 'resume' | 'cancel' | 'retry';

function ok<T>(value: T): StoreResult<T> {
  return { ok: true, value };
}

function fail(status: number, code: string, message: string): StoreFailure {
  return { ok: false, error: { status, code, message } };
}

function unauthorized(): StoreFailure {
  return fail(401, 'UNAUTHORIZED', '未登录或会话已失效');
}

function forbidden(message: string): StoreFailure {
  return fail(403, 'FORBIDDEN', message);
}

function notFound(message: string): StoreFailure {
  return fail(404, 'NOT_FOUND', message);
}

// 分页默认值与上限（NFR-16）
const DEFAULT_PAGE_SIZE = 50;
const MAX_PAGE_SIZE = 200;

/** 分页响应通用结构（与契约各 *ListResponse 字段一致）。 */
interface Paged<T> {
  items: T[];
  page: number;
  page_size: number;
  total: number;
}

/** 纯函数分页：page 从 1 起，page_size 默认 50、上限 200。 */
function paginate<T>(all: T[], page?: number, pageSize?: number): Paged<T> {
  const size = Math.min(MAX_PAGE_SIZE, Math.max(1, pageSize ?? DEFAULT_PAGE_SIZE));
  const current = Math.max(1, page ?? 1);
  const start = (current - 1) * size;
  return {
    items: all.slice(start, start + size),
    page: current,
    page_size: size,
    total: all.length,
  };
}

/** 当前时间戳（新建实体用）。 */
function now(): string {
  return new Date().toISOString();
}

/** 权限档位向下包含：admin ⊃ write ⊃ read。 */
function permissionCovers(granted: Permission, needed: Permission): boolean {
  const rank: Record<Permission, number> = { read: 1, write: 2, admin: 3 };
  return rank[granted] >= rank[needed];
}

/** 归一化目录路径：去掉首尾斜杠，空串表示仓库根。 */
function normalizeDir(path: string): string {
  return path.replace(/^\/+/, '').replace(/\/+$/, '');
}

/** 列出目录直接子项：子目录在前、文件在后，各按名称排序。 */
function listDirectory(files: SeedFile[], dir: string): FileEntry[] {
  const prefix = dir === '' ? '' : `${dir}/`;
  const subdirs = new Set<string>();
  const fileEntries: FileEntry[] = [];
  for (const file of files) {
    if (!file.path.startsWith(prefix)) continue;
    const rest = file.path.slice(prefix.length);
    const slash = rest.indexOf('/');
    if (slash < 0) {
      fileEntries.push({
        name: rest,
        is_dir: false,
        size: file.size,
        sha256: file.sha256,
        updated_at: file.updated_at,
      });
    } else {
      subdirs.add(rest.slice(0, slash));
    }
  }
  const dirEntries = [...subdirs].sort().map((name): FileEntry => ({ name, is_dir: true }));
  fileEntries.sort((a, b) => a.name.localeCompare(b.name));
  return [...dirEntries, ...fileEntries];
}

/** 按格式生成使用方式片段：凭据一律 <令牌> 占位，不出现任何真实凭据。 */
function usageSnippets(repo: RepoInfo): UsageSnippet[] {
  const base = `https://<服务器地址>/repos/${repo.name}`;
  if (repo.format === 'maven') {
    return [
      {
        title: 'Maven settings.xml',
        content: `<server>\n  <id>${repo.name}</id>\n  <username><用户名></username>\n  <password><令牌></password>\n</server>`,
      },
      {
        title: 'pom.xml 仓库配置',
        content: `<repository>\n  <id>${repo.name}</id>\n  <url>${base}</url>\n</repository>`,
      },
      {
        title: 'mvn 部署命令',
        content: `mvn deploy -DaltDeploymentRepository=${repo.name}::${base}`,
      },
    ];
  }
  if (repo.format === 'npm') {
    return [
      {
        title: '.npmrc 配置',
        content: `registry=${base}/\n//<服务器地址>/repos/${repo.name}/:_authToken=<令牌>`,
      },
      {
        title: 'npm 安装与发布',
        content: `npm install --registry=${base}/\nnpm publish --registry=${base}/`,
      },
    ];
  }
  return [
    {
      title: 'curl 上传',
      content: `curl -u <用户名>:<令牌> -T ./文件.bin ${base}/文件.bin`,
    },
    {
      title: 'curl 下载',
      content: `curl -u <用户名>:<令牌> -O ${base}/文件.bin`,
    },
  ];
}

/** 来源必填项校验：native-dir 需 path 与 config_source，online 需 url，package 需 path。 */
function validateSource(source: MigrationSource): StoreFailure | null {
  if (source.kind === 'native-dir' && (!source.path || !source.config_source)) {
    return fail(400, 'INVALID_SOURCE', 'native-dir 来源必须提供 path 与 config_source');
  }
  if (source.kind === 'online' && !source.url) {
    return fail(400, 'INVALID_SOURCE', 'online 来源必须提供 url');
  }
  if (source.kind === 'package' && !source.path) {
    return fail(400, 'INVALID_SOURCE', 'package 来源必须提供 path');
  }
  return null;
}

/** 发现结果的固定样本仓库（名称与 default 场景既有仓库部分重合，便于演示冲突）。 */
function sampleDiscoveredRepos(): DiscoveredRepo[] {
  return [
    {
      // 源 Nexus 仓库名，刻意与目标实例既有仓库不撞名（迁移是搬入新仓库，默认无冲突）
      name: 'maven-legacy',
      format: 'maven',
      repo_type: 'hosted',
      has_credential_ref: false,
      object_count: 1280,
      total_bytes: 2_147_483_648,
      config_complete: true,
      members: null,
      upstream_url: null,
    },
    {
      name: 'npm-mirror',
      format: 'npm',
      repo_type: 'proxy',
      has_credential_ref: true,
      object_count: 356,
      total_bytes: 734_003_200,
      config_complete: true,
      members: null,
      upstream_url: 'https://registry.npmjs.org',
    },
    {
      name: 'raw-archive',
      format: 'raw',
      repo_type: 'hosted',
      has_credential_ref: false,
      object_count: 42,
      total_bytes: 10_485_760,
      config_complete: true,
      members: null,
      upstream_url: null,
    },
  ];
}

/** 控制动作的合法前置状态与目标状态（十态状态机的合法转移，M1-FR-028）。 */
const TASK_ACTIONS: Record<TaskAction, { from: TaskStatus[]; to: TaskStatus }> = {
  pause: { from: ['running'], to: 'paused' },
  resume: { from: ['paused'], to: 'running' },
  cancel: { from: ['queued', 'running', 'pausing', 'paused'], to: 'cancelled' },
  retry: { from: ['failed', 'partial', 'interrupted', 'cancelled'], to: 'running' },
};

/** 控制动作的中文名（错误消息用）。 */
const ACTION_LABEL: Record<TaskAction, string> = {
  pause: '暂停',
  resume: '继续',
  cancel: '取消',
  retry: '重试',
};

/** 推进 running 任务进度：每次轮询前进约五分之一，完成即转 succeeded。 */
function advanceProgress(info: TaskInfo): void {
  if (info.status !== 'running' || info.total_objects <= 0) return;
  const step = Math.max(1, Math.floor(info.total_objects / 5));
  info.done_objects = Math.min(info.total_objects, info.done_objects + step);
  info.done_bytes = Math.floor((info.done_objects / info.total_objects) * info.total_bytes);
  info.updated_at = now();
  if (info.done_objects >= info.total_objects) info.status = 'succeeded';
}

/** 按任务当前状态合成基础报告（无预置报告的任务用）。 */
function synthesizeReport(info: TaskInfo): MigrationReport {
  return {
    task_id: info.task_id,
    status: info.status,
    repositories: [
      {
        source: 'maven-releases',
        target: 'maven-releases',
        total: info.total_objects,
        done: info.done_objects,
        migrated: info.done_objects,
        skipped: 0,
        failed: 0,
        total_bytes: info.total_bytes,
        done_bytes: info.done_bytes,
        duration_ms: 60_000,
        credential_status: '已映射',
      },
    ],
    failures: [],
    failures_omitted: 0,
    unsupported: [],
    excluded: [],
  };
}

/** 有状态内存数据模型：由场景种子构造，支持原地切换场景。 */
export class DevmockStore {
  // 全部字段在 resetTo 中按种子覆盖（构造函数即调用），声明处仅给零值缺省
  private scenario: ScenarioId = 'default';
  private users: SeedUser[] = [];
  private tokens: SeedToken[] = [];
  private repos: SeedRepo[] = [];
  private tasks: SeedTask[] = [];
  private session: { userId: number } | null = null;
  private upstreamDown = false;
  private readonly discoveries = new Map<string, DiscoveryResult>();
  private readonly plans = new Map<string, MigrationPlan>();
  private seq = 0;

  // 迁移任务持久化键：模拟后端对任务的持久化（FR-029），使「刷新找回」跨页面重载成立。
  private static readonly TASKS_KEY = 'jianartifact.devmock.tasks';

  constructor(scenario: ScenarioId) {
    this.resetTo(scenario);
    // 页面重载后 rehydrate 已持久化的运行时任务（scenario 种子任务不受影响，按 id 去重）
    this.rehydrateTasks();
  }

  /** 浏览器环境才有 localStorage；node 测试环境跳过持久化（用例不涉及重载）。 */
  private static hasStorage(): boolean {
    return typeof localStorage !== 'undefined' && localStorage !== null;
  }

  /** 把当前任务列表写入 localStorage，模拟后端持久化。 */
  private persistTasks(): void {
    if (!DevmockStore.hasStorage()) return;
    try {
      localStorage.setItem(DevmockStore.TASKS_KEY, JSON.stringify(this.tasks));
    } catch {
      // 存储不可用（隐私模式/配额）时静默降级，不影响交互
    }
  }

  /** 从 localStorage 追加已持久化的任务（按 task_id 去重，种子任务优先保留）。 */
  private rehydrateTasks(): void {
    if (!DevmockStore.hasStorage()) return;
    try {
      const raw = localStorage.getItem(DevmockStore.TASKS_KEY);
      if (!raw) return;
      const persisted = JSON.parse(raw) as SeedTask[];
      const known = new Set(this.tasks.map((t) => t.info.task_id));
      for (const task of persisted) {
        if (!known.has(task.info.task_id)) this.tasks.push(task);
      }
    } catch {
      // 持久化内容损坏时忽略，回到纯种子状态
    }
  }

  /** 清除持久化任务：显式切换场景视为全新后端。 */
  clearPersistedTasks(): void {
    if (!DevmockStore.hasStorage()) return;
    try {
      localStorage.removeItem(DevmockStore.TASKS_KEY);
    } catch {
      // 忽略清除失败
    }
  }

  /** 当前场景标识。 */
  get scenarioId(): ScenarioId {
    return this.scenario;
  }

  /** 切换场景：原地重建为目标场景的种子状态（实例引用保持不变）。 */
  resetTo(scenario: ScenarioId): void {
    const seed = seedFor(scenario);
    this.scenario = scenario;
    this.users = seed.users;
    this.tokens = seed.tokens;
    this.repos = seed.repos;
    this.tasks = seed.tasks;
    this.session = seed.session;
    this.upstreamDown = seed.upstreamDown;
    this.discoveries.clear();
    this.plans.clear();
    this.seq = 0;
  }

  /** 就绪自检：上游故障场景视为存在未就绪依赖。 */
  isReady(): boolean {
    return !this.upstreamDown;
  }

  // —— 会话 ——

  /** 登录：校验测试凭据并建立会话（default 场景凭据 admin/admin123、dev/dev123）。 */
  login(username: string, password: string): StoreResult<SessionToken> {
    const user = this.users.find((u) => u.info.username === username);
    if (!user || !user.info.enabled || user.password !== password) {
      return fail(401, 'INVALID_CREDENTIALS', '用户名或口令无效');
    }
    this.session = { userId: user.info.id };
    return ok(this.issueSessionToken());
  }

  /** 退出：吊销当前会话。 */
  logout(): StoreResult<null> {
    if (!this.session) return unauthorized();
    this.session = null;
    return ok(null);
  }

  /** 刷新：换发新会话令牌。 */
  refresh(): StoreResult<SessionToken> {
    if (!this.session) return unauthorized();
    return ok(this.issueSessionToken());
  }

  /** 生成伪会话令牌（纯 mock 值，无任何真实密钥参与）。 */
  private issueSessionToken(): SessionToken {
    this.seq += 1;
    return {
      token: `jwt_mock_${String(this.seq)}_${Math.random().toString(36).slice(2, 10)}`,
      expires_in: 3600,
    };
  }

  /** 当前会话用户（null = 未登录）。 */
  private currentUser(): SeedUser | null {
    const session = this.session;
    if (!session) return null;
    return this.users.find((u) => u.info.id === session.userId) ?? null;
  }

  /** 会话守卫：未登录返回 401。 */
  private requireUser(): StoreResult<SeedUser> {
    const user = this.currentUser();
    return user ? ok(user) : unauthorized();
  }

  /** 管理员守卫：未登录 401，非管理员 403。 */
  private requireAdmin(): StoreResult<SeedUser> {
    const user = this.requireUser();
    if (!user.ok) return user;
    return user.value.info.role === 'admin' ? user : forbidden('需要管理员权限');
  }

  // —— Token ——

  /** 列出本人 Token 元数据（分页）。 */
  listTokens(page?: number, pageSize?: number): StoreResult<TokenListResponse> {
    const user = this.requireUser();
    if (!user.ok) return user;
    const own = this.tokens.filter((t) => t.userId === user.value.info.id).map((t) => t.info);
    return ok(paginate(own, page, pageSize));
  }

  /** 签发 Token：明文仅在本次返回中出现一次，存储只留元数据。 */
  createToken(name: string): StoreResult<CreatedToken> {
    const user = this.requireUser();
    if (!user.ok) return user;
    const id = this.tokens.reduce((max, t) => Math.max(max, t.info.id), 0) + 1;
    const info: TokenInfo = { id, name, created_at: now() };
    this.tokens.push({ info, userId: user.value.info.id });
    return ok({
      ...info,
      token: `jat_mock_${String(id)}_${Math.random().toString(36).slice(2, 10)}`,
    });
  }

  /** 吊销本人 Token。 */
  revokeToken(id: number): StoreResult<null> {
    const user = this.requireUser();
    if (!user.ok) return user;
    const uid = user.value.info.id;
    const index = this.tokens.findIndex((t) => t.info.id === id && t.userId === uid);
    if (index < 0) return notFound('Token 不存在');
    this.tokens.splice(index, 1);
    return ok(null);
  }

  // —— 用户管理（Admin）——

  /** 列出用户（分页，Admin）。 */
  listUsers(page?: number, pageSize?: number): StoreResult<UserListResponse> {
    const admin = this.requireAdmin();
    if (!admin.ok) return admin;
    const items = this.users.map((u) => u.info);
    return ok(paginate(items, page, pageSize));
  }

  /** 新增用户（Admin）。 */
  createUser(req: CreateUserRequest): StoreResult<UserInfo> {
    const admin = this.requireAdmin();
    if (!admin.ok) return admin;
    if (this.users.some((u) => u.info.username === req.username)) {
      return fail(409, 'USERNAME_TAKEN', '用户名已存在');
    }
    const id = this.users.reduce((max, u) => Math.max(max, u.info.id), 0) + 1;
    const info: UserInfo = {
      id,
      username: req.username,
      role: req.role,
      enabled: true,
      created_at: now(),
    };
    this.users.push({ info, password: req.password });
    return ok(info);
  }

  /** 更新用户启停与角色：含「最后一名启用管理员不可禁用/降级」保护。 */
  updateUser(id: number, req: UpdateUserRequest): StoreResult<UserInfo> {
    const admin = this.requireAdmin();
    if (!admin.ok) return admin;
    const user = this.users.find((u) => u.info.id === id);
    if (!user) return notFound('用户不存在');
    if (this.wouldRemoveLastAdmin(user.info, req)) {
      return fail(403, 'LAST_ADMIN_PROTECTED', '不能禁用或降级最后一名启用的管理员');
    }
    if (req.enabled != null) user.info.enabled = req.enabled;
    if (req.role != null) user.info.role = req.role;
    return ok(user.info);
  }

  /** 判断该更新是否会移除最后一名启用管理员。 */
  private wouldRemoveLastAdmin(target: UserInfo, req: UpdateUserRequest): boolean {
    if (!(target.role === 'admin' && target.enabled)) return false;
    const demoted = req.enabled === false || (req.role != null && req.role !== 'admin');
    if (!demoted) return false;
    const enabledAdmins = this.users.filter((u) => u.info.role === 'admin' && u.info.enabled);
    return enabledAdmins.length <= 1;
  }

  // —— 仓库 ——

  /** 调用方对仓库的权限：admin 全量；ACL 命中取其档位；public 兜底 read。 */
  private permissionFor(repo: SeedRepo): Permission | null {
    const user = this.currentUser();
    if (user?.info.role === 'admin') return 'admin';
    if (user) {
      const entry = repo.acl.find((a) => a.user_id === user.info.id);
      if (entry) return entry.permission;
    }
    return repo.info.visibility === 'public' ? 'read' : null;
  }

  /** 仓库守卫：要求指定权限档位；不可见与不存在同样返回 404（零存在性泄露）。 */
  private requireRepo(name: string, needed: Permission): StoreResult<SeedRepo> {
    const repo = this.repos.find((r) => r.info.name === name);
    if (!repo) return notFound('仓库不存在');
    const granted = this.permissionFor(repo);
    if (!granted) return notFound('仓库不存在');
    if (!permissionCovers(granted, needed)) return forbidden('无该仓库的操作权限');
    return ok(repo);
  }

  /** 仓库列表：只含调用方可读仓库（分页）。 */
  listRepos(page?: number, pageSize?: number): StoreResult<RepoListResponse> {
    const visible = this.repos.filter((r) => this.permissionFor(r) !== null).map((r) => r.info);
    return ok(paginate(visible, page, pageSize));
  }

  /** 仓库详情。 */
  getRepo(name: string): StoreResult<RepoInfo> {
    const repo = this.requireRepo(name, 'read');
    return repo.ok ? ok(repo.value.info) : repo;
  }

  /** 创建仓库（Admin）。 */
  createRepo(req: CreateRepoRequest): StoreResult<RepoInfo> {
    const admin = this.requireAdmin();
    if (!admin.ok) return admin;
    if (this.repos.some((r) => r.info.name === req.name)) {
      return fail(409, 'REPO_EXISTS', '仓库名已存在');
    }
    const info: RepoInfo = {
      name: req.name,
      format: req.format,
      repo_type: req.repo_type,
      visibility: req.visibility,
      disabled: false,
      created_at: now(),
      members: req.members ?? null,
      upstream_url: req.upstream_url ?? null,
      upstream_credential_ref: req.upstream_credential_ref ?? null,
      overwrite_policy: req.overwrite_policy ?? null,
      version_policy: req.version_policy ?? null,
    };
    this.repos.push({ info, acl: [], files: [] });
    return ok(info);
  }

  /** 编辑仓库：只更新给出的字段；format 与 repo_type 不可变。 */
  updateRepo(name: string, req: UpdateRepoRequest): StoreResult<RepoInfo> {
    const repo = this.requireRepo(name, 'admin');
    if (!repo.ok) return repo;
    const info = repo.value.info;
    if (req.visibility != null) info.visibility = req.visibility;
    if (req.disabled != null) info.disabled = req.disabled;
    if (req.members != null) info.members = req.members;
    if (req.upstream_url != null) info.upstream_url = req.upstream_url;
    if (req.upstream_credential_ref != null) {
      info.upstream_credential_ref = req.upstream_credential_ref;
    }
    if (req.overwrite_policy != null) info.overwrite_policy = req.overwrite_policy;
    return ok(info);
  }

  /** 删除仓库：需显式模式与确认头（X-Confirm-Repository=仓库名）。 */
  deleteRepo(
    name: string,
    mode: string | null,
    confirm: string | null,
  ): StoreResult<DeleteRepoResponse> {
    const repo = this.requireRepo(name, 'admin');
    if (!repo.ok) return repo;
    if (mode !== 'config' && mode !== 'data') {
      return fail(400, 'INVALID_DELETE_MODE', '必须显式给出删除模式 config 或 data');
    }
    if (confirm !== name) {
      return fail(400, 'CONFIRM_MISMATCH', '确认头必须与仓库名一致');
    }
    const blobCount = repo.value.files.length;
    this.repos = this.repos.filter((r) => r.info.name !== name);
    if (mode === 'data') return ok({ reclaimed_blobs: blobCount, orphaned_blobs: 0 });
    return ok({ reclaimed_blobs: 0, orphaned_blobs: blobCount });
  }

  /** ACL 条目列表（仓库 admin）。 */
  aclList(name: string): StoreResult<AclEntry[]> {
    const repo = this.requireRepo(name, 'admin');
    return repo.ok ? ok(repo.value.acl) : repo;
  }

  /** 设置某用户的 ACL（仓库 admin）。 */
  aclSet(name: string, userId: number, permission: Permission): StoreResult<AclEntry> {
    const repo = this.requireRepo(name, 'admin');
    if (!repo.ok) return repo;
    const user = this.users.find((u) => u.info.id === userId);
    if (!user) return notFound('用户不存在');
    const entry: AclEntry = { user_id: userId, username: user.info.username, permission };
    const acl = repo.value.acl;
    const index = acl.findIndex((a) => a.user_id === userId);
    if (index >= 0) acl[index] = entry;
    else acl.push(entry);
    return ok(entry);
  }

  /** 移除某用户的 ACL（仓库 admin）。 */
  aclRemove(name: string, userId: number): StoreResult<null> {
    const repo = this.requireRepo(name, 'admin');
    if (!repo.ok) return repo;
    const index = repo.value.acl.findIndex((a) => a.user_id === userId);
    if (index < 0) return notFound('ACL 条目不存在');
    repo.value.acl.splice(index, 1);
    return ok(null);
  }

  /** 上游故障守卫：仅对 proxy 仓库且场景标记故障时生效。 */
  private checkUpstream(repo: SeedRepo): StoreFailure | null {
    if (this.upstreamDown && repo.info.repo_type === 'proxy') {
      return fail(502, 'UPSTREAM_UNAVAILABLE', '上游服务不可用');
    }
    return null;
  }

  /** 目录浏览（read 权限，分页）。 */
  listFiles(
    name: string,
    path: string,
    page?: number,
    pageSize?: number,
  ): StoreResult<FileListResponse> {
    const repo = this.requireRepo(name, 'read');
    if (!repo.ok) return repo;
    const down = this.checkUpstream(repo.value);
    if (down) return down;
    const dir = normalizeDir(path);
    const entries = listDirectory(repo.value.files, dir);
    if (dir !== '' && entries.length === 0) return notFound('路径不存在');
    return ok(paginate(entries, page, pageSize));
  }

  /** 仓库内路径搜索（read 权限，子串匹配，分页；条目名为完整路径）。 */
  search(name: string, q: string, page?: number, pageSize?: number): StoreResult<FileListResponse> {
    const repo = this.requireRepo(name, 'read');
    if (!repo.ok) return repo;
    const down = this.checkUpstream(repo.value);
    if (down) return down;
    const matches: FileEntry[] = [];
    for (const f of repo.value.files) {
      if (!f.path.includes(q)) continue;
      matches.push({
        name: f.path,
        is_dir: false,
        size: f.size,
        sha256: f.sha256,
        updated_at: f.updated_at,
      });
    }
    return ok(paginate(matches, page, pageSize));
  }

  /** 使用方式片段（read 权限）。 */
  usage(name: string): StoreResult<UsageSnippet[]> {
    const repo = this.requireRepo(name, 'read');
    return repo.ok ? ok(usageSnippets(repo.value.info)) : repo;
  }

  // —— 迁移（Admin）——

  /** 发现：按来源语义校验必填项；上游故障场景在线发现失败。 */
  discover(source: MigrationSource): StoreResult<DiscoveryResult> {
    const admin = this.requireAdmin();
    if (!admin.ok) return admin;
    const invalid = validateSource(source);
    if (invalid) return invalid;
    if (source.kind === 'online' && this.upstreamDown) {
      return fail(502, 'UPSTREAM_UNAVAILABLE', '无法连接 Nexus 实例，在线发现失败');
    }
    this.seq += 1;
    const result: DiscoveryResult = {
      discovery_id: `disc_${String(this.seq)}`,
      snapshot_time: now(),
      degraded_bare_blob: false,
      repositories: sampleDiscoveredRepos(),
      unsupported: [
        {
          name: 'docker-local',
          category: '非受支持格式',
          reason: 'Docker 格式属后续里程碑，未纳入迁移矩阵',
        },
      ],
    };
    this.discoveries.set(result.discovery_id, result);
    return ok(result);
  }

  /** 计算映射冲突：目标名与既有仓库重名即阻断。 */
  private conflictsFor(mappings: PlanRequest['mappings']): PlanConflict[] {
    const conflicts: PlanConflict[] = [];
    for (const mapping of mappings) {
      if (mapping.excluded) continue;
      const target = mapping.target ?? mapping.source;
      if (this.repos.some((r) => r.info.name === target)) {
        conflicts.push({
          target,
          path: '',
          category: '目标仓库已存在',
          detail: `目标仓库 ${target} 已存在，请改名映射或显式排除`,
        });
      }
    }
    return conflicts;
  }

  /** 生成迁移计划预览：目标重名仓库产生阻断性冲突。 */
  plan(req: PlanRequest): StoreResult<MigrationPlan> {
    const admin = this.requireAdmin();
    if (!admin.ok) return admin;
    const discovery = this.discoveries.get(req.discovery_id);
    if (!discovery) return fail(400, 'DISCOVERY_NOT_FOUND', '引用的发现结果不存在');
    const conflicts = this.conflictsFor(req.mappings);
    const included = req.mappings.filter((m) => !m.excluded);
    const sources = new Set(included.map((m) => m.source));
    const chosen = discovery.repositories.filter((r) => sources.has(r.name));
    const totalBytes = chosen.reduce((sum, r) => sum + r.total_bytes, 0);
    this.seq += 1;
    const result: MigrationPlan = {
      plan_id: `plan_${String(this.seq)}`,
      mappings: req.mappings,
      conflicts,
      blocked: conflicts.length > 0,
      object_count: chosen.reduce((sum, r) => sum + r.object_count, 0),
      total_bytes: totalBytes,
      estimated_disk_bytes: Math.ceil(totalBytes * 1.2),
      disk_sufficient: true,
      unsupported: discovery.unsupported,
    };
    this.plans.set(result.plan_id, result);
    return ok(result);
  }

  /** 创建任务：计划必须存在且无阻断冲突；受理即进入 running 供进度轮询。 */
  createTask(planId: string): StoreResult<TaskInfo> {
    const admin = this.requireAdmin();
    if (!admin.ok) return admin;
    const plan = this.plans.get(planId);
    if (!plan) return fail(400, 'PLAN_NOT_FOUND', '计划不存在');
    if (plan.blocked) return fail(400, 'PLAN_BLOCKED', '计划存在未决阻断冲突，不能执行');
    this.seq += 1;
    const info: TaskInfo = {
      task_id: `task_${String(this.seq)}`,
      plan_id: planId,
      status: 'running',
      done_objects: 0,
      total_objects: plan.object_count,
      done_bytes: 0,
      total_bytes: plan.total_bytes,
      created_at: now(),
      updated_at: now(),
    };
    this.tasks.push({ info, report: null });
    this.persistTasks();
    return ok(info);
  }

  /** 任务列表（分页，Admin）。 */
  listTasks(page?: number, pageSize?: number): StoreResult<TaskListResponse> {
    const admin = this.requireAdmin();
    if (!admin.ok) return admin;
    const items = this.tasks.map((t) => t.info);
    return ok(paginate(items, page, pageSize));
  }

  /** 任务详情：running 任务每次轮询推进进度，直至完成转 succeeded。 */
  getTask(id: string): StoreResult<TaskInfo> {
    const admin = this.requireAdmin();
    if (!admin.ok) return admin;
    const task = this.tasks.find((t) => t.info.task_id === id);
    if (!task) return notFound('任务不存在');
    advanceProgress(task.info);
    this.persistTasks();
    return ok(task.info);
  }

  /** 任务控制动作：按状态机合法转移执行，非法转移返回 409。 */
  controlTask(id: string, action: TaskAction): StoreResult<TaskInfo> {
    const admin = this.requireAdmin();
    if (!admin.ok) return admin;
    const task = this.tasks.find((t) => t.info.task_id === id);
    if (!task) return notFound('任务不存在');
    const rule = TASK_ACTIONS[action];
    if (!rule.from.includes(task.info.status)) {
      return fail(409, 'ILLEGAL_TRANSITION', `当前状态不允许${ACTION_LABEL[action]}`);
    }
    task.info.status = rule.to;
    task.info.updated_at = now();
    this.persistTasks();
    return ok(task.info);
  }

  /** 任务报告：预置报告优先（migration-partial 场景），否则按当前状态合成。 */
  getReport(id: string): StoreResult<MigrationReport> {
    const admin = this.requireAdmin();
    if (!admin.ok) return admin;
    const task = this.tasks.find((t) => t.info.task_id === id);
    if (!task) return notFound('任务不存在');
    return ok(task.report ?? synthesizeReport(task.info));
  }
}

/** 按场景创建全新的数据模型实例。 */
export function createStore(id: ScenarioId): DevmockStore {
  return new DevmockStore(id);
}

/** 切换场景：清除持久化任务（视为全新后端）并原地重建为目标场景种子（模块级引用保持不变）。 */
export function switchScenario(store: DevmockStore, id: ScenarioId): void {
  store.clearPersistedTasks();
  store.resetTo(id);
}
