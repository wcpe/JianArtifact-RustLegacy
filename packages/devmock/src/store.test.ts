// 数据模型与场景行为测试（M1-FR-035）：会话、Token、用户保护、仓库权限、分页与迁移状态机。
import { beforeAll, describe, expect, it } from 'vitest';
import { DEVMOCK_SETUP_TOKEN } from './scenarios';
import { createStore, type StoreResult } from './store';

/** 断言成功结果并取出载荷。 */
function unwrap<T>(result: StoreResult<T>): T {
  if (!result.ok) throw new Error(`预期成功结果，实际失败：${result.error.code}`);
  return result.value;
}

/** 断言失败结果并取出错误语义。 */
function unwrapError<T>(result: StoreResult<T>) {
  if (result.ok) throw new Error('预期失败结果，实际成功');
  return result.error;
}

describe('首启管理员初始化', () => {
  it('setup-required 场景报告需初始化，成功后原子关闭且令牌不可复用', () => {
    const store = createStore('setup-required');
    expect(unwrap(store.setupStatus()).setup_required).toBe(true);

    const created = store.setup(DEVMOCK_SETUP_TOKEN, '  first-admin  ', '密'.repeat(12));
    expect(created.ok).toBe(true);
    expect(unwrap(store.setupStatus()).setup_required).toBe(false);
    expect(unwrapError(store.setup(DEVMOCK_SETUP_TOKEN, 'other-admin', '密'.repeat(12))).code).toBe(
      'SETUP_UNAVAILABLE',
    );
    expect(store.login('first-admin', '密'.repeat(12)).ok).toBe(true);
  });

  it('用户名与 Unicode 口令边界返回稳定错误码', () => {
    const store = createStore('setup-required');
    expect(unwrapError(store.setup(DEVMOCK_SETUP_TOKEN, '   ', '密'.repeat(12))).code).toBe(
      'SETUP_USERNAME_REQUIRED',
    );
    expect(unwrapError(store.setup(DEVMOCK_SETUP_TOKEN, 'admin', '密'.repeat(11))).code).toBe(
      'SETUP_PASSWORD_TOO_SHORT',
    );
  });
});

describe('会话流转', () => {
  it('退出后受保护操作返回 401，重新登录恢复会话', () => {
    const store = createStore('default');
    expect(unwrap(store.refresh()).token).toMatch(/^jwt_mock_/);
    expect(store.logout().ok).toBe(true);
    expect(unwrapError(store.refresh()).status).toBe(401);
    expect(unwrapError(store.listTokens()).status).toBe(401);
    const token = unwrap(store.login('dev', 'dev123'));
    expect(token.expires_in).toBeGreaterThan(0);
    expect(store.refresh().ok).toBe(true);
  });

  it('错误凭据与未知用户登录均返回统一认证错误', () => {
    const store = createStore('default');
    store.logout();
    expect(unwrapError(store.me()).code).toBe('AUTH_UNAUTHENTICATED');
    expect(unwrapError(store.login('admin', '错误口令')).code).toBe('AUTH_INVALID_CREDENTIALS');
    expect(unwrapError(store.login('ghost', 'x')).code).toBe('AUTH_INVALID_CREDENTIALS');
  });
});

describe('Token 管理', () => {
  it('明文仅创建时返回一次，列表不含明文，吊销后消失', () => {
    const store = createStore('default');
    const created = unwrap(store.createToken('发布用'));
    expect(created.token.startsWith('jat_mock_')).toBe(true);
    const listed = unwrap(store.listTokens());
    const found = listed.items.find((t) => t.id === created.id);
    expect(found?.name).toBe('发布用');
    expect(found !== undefined && 'token' in found).toBe(false);
    expect(store.revokeToken(created.id).ok).toBe(true);
    expect(unwrap(store.listTokens()).items.some((t) => t.id === created.id)).toBe(false);
    expect(unwrapError(store.revokeToken(created.id)).code).toBe('AUTH_TOKEN_NOT_FOUND');
  });
});

describe('用户管理', () => {
  it('最后一名启用管理员不可禁用或降级；补充管理员后可禁用', () => {
    const store = createStore('default');
    expect(unwrapError(store.updateUser(1, { enabled: false })).code).toBe('AUTH_LAST_ADMIN');
    expect(unwrapError(store.updateUser(1, { role: 'user' })).code).toBe('AUTH_LAST_ADMIN');
    unwrap(store.createUser({ username: 'admin2', password: 'admin2pass', role: 'admin' }));
    expect(store.updateUser(1, { enabled: false }).ok).toBe(true);
  });

  it('restricted 场景下普通用户访问用户管理被拒（403）', () => {
    const store = createStore('restricted');
    expect(unwrapError(store.listUsers()).code).toBe('AUTH_FORBIDDEN');
    const created = store.createUser({ username: 'x', password: 'p', role: 'user' });
    expect(unwrapError(created).code).toBe('AUTH_FORBIDDEN');
  });

  it('用户域错误码与后端 AUTH_ 前缀一致', () => {
    const store = createStore('default');
    const duplicate = store.createUser({ username: 'admin', password: 'p', role: 'user' });
    expect(unwrapError(duplicate).code).toBe('AUTH_USERNAME_TAKEN');
    expect(unwrapError(store.updateUser(999, { enabled: false })).code).toBe('AUTH_USER_NOT_FOUND');
  });

  it('修改本人口令：旧口令错误，成功后旧口令与当前会话立即失效（M1-FR-045）', () => {
    const store = createStore('default');
    expect(unwrapError(store.changePassword('错误口令', 'new-pass')).code).toBe(
      'AUTH_INVALID_CREDENTIALS',
    );
    expect(store.changePassword('admin123', 'new-pass').ok).toBe(true);
    expect(unwrapError(store.me()).code).toBe('AUTH_UNAUTHENTICATED');
    expect(unwrapError(store.login('admin', 'admin123')).code).toBe('AUTH_INVALID_CREDENTIALS');
    expect(store.login('admin', 'new-pass').ok).toBe(true);
  });

  it('Admin 重置用户口令后立即生效，重置当前用户时撤销会话（M1-FR-045）', () => {
    const store = createStore('default');
    expect(store.updateUser(2, { password: 'reset-pass' }).ok).toBe(true);
    expect(store.me().ok).toBe(true);
    expect(unwrapError(store.login('dev', 'dev123')).code).toBe('AUTH_INVALID_CREDENTIALS');
    expect(store.login('dev', 'reset-pass').ok).toBe(true);

    const adminStore = createStore('default');
    expect(adminStore.updateUser(1, { password: 'admin-new' }).ok).toBe(true);
    expect(unwrapError(adminStore.me()).code).toBe('AUTH_UNAUTHENTICATED');
  });
});

describe('仓库管理', () => {
  it('创建后列表可见，重名 409，删除后消失', () => {
    const store = createStore('empty');
    expect(unwrap(store.listRepos()).total).toBe(0);
    const req = {
      name: 'demo-raw',
      format: 'raw',
      repo_type: 'hosted',
      visibility: 'public',
    } as const;
    unwrap(store.createRepo(req));
    expect(unwrap(store.listRepos()).items.map((r) => r.name)).toContain('demo-raw');
    expect(unwrapError(store.createRepo(req)).status).toBe(409);
    unwrap(store.deleteRepo('demo-raw', 'config', 'demo-raw'));
    expect(unwrap(store.listRepos()).total).toBe(0);
  });

  it('删除必须显式给出模式与匹配的确认头，data 模式回收 blob 计数', () => {
    const store = createStore('default');
    expect(unwrapError(store.deleteRepo('raw-hosted', 'data', null)).status).toBe(400);
    expect(unwrapError(store.deleteRepo('raw-hosted', null, 'raw-hosted')).status).toBe(400);
    const removed = unwrap(store.deleteRepo('raw-hosted', 'data', 'raw-hosted'));
    expect(removed.reclaimed_blobs).toBe(3);
  });

  it('restricted 场景：无权 private 仓库不出现在列表且详情按不存在处理（零泄露）', () => {
    const store = createStore('restricted');
    const names = unwrap(store.listRepos()).items.map((r) => r.name);
    expect(names).not.toContain('npm-internal');
    expect(names).toContain('maven-releases');
    expect(unwrapError(store.getRepo('npm-internal')).status).toBe(404);
  });

  it('使用方式片段：凭据一律为占位符，不含任何 mock 明文', () => {
    const store = createStore('default');
    const snippets = unwrap(store.usage('npm-proxy'));
    expect(snippets.length).toBeGreaterThanOrEqual(2);
    for (const snippet of snippets) {
      expect(snippet.content).not.toContain('admin123');
      expect(snippet.content).not.toContain('jat_mock_');
    }
    expect(snippets.some((s) => s.content.includes('<令牌>'))).toBe(true);
  });

  it('ACL 以用户名为精确键设置与移除', () => {
    const store = createStore('default');
    const setByUsername = store.aclSet as unknown as (
      name: string,
      username: string,
      permission: 'read' | 'write' | 'admin',
    ) => ReturnType<typeof store.aclSet>;
    const removeByUsername = store.aclRemove as unknown as (
      name: string,
      username: string,
    ) => ReturnType<typeof store.aclRemove>;

    const entry = unwrap(setByUsername.call(store, 'raw-hosted', 'dev', 'write'));
    expect(entry).toMatchObject({ user_id: 2, username: 'dev', permission: 'write' });
    expect(removeByUsername.call(store, 'raw-hosted', 'dev').ok).toBe(true);
  });

  it('PATCH 可显式清空上游凭据引用并更新 Maven 版本策略', () => {
    const store = createStore('default');
    const cleared = unwrap(store.updateRepo('npm-proxy', { upstream_credential_ref: null }));
    expect(cleared.upstream_credential_ref).toBeNull();

    const updateWithVersionPolicy = {
      version_policy: 'snapshot',
    } as Parameters<typeof store.updateRepo>[1];
    const updated = unwrap(store.updateRepo('maven-releases', updateWithVersionPolicy));
    expect(updated.version_policy).toBe('snapshot');
  });

  it('浏览与搜索条目均返回仓库根相对完整路径', () => {
    const store = createStore('default');
    const root = unwrap(store.listFiles('raw-hosted', ''));
    expect(root.items.find((entry) => entry.name === 'docs')?.path).toBe('docs');
    const docs = unwrap(store.listFiles('raw-hosted', 'docs'));
    expect(docs.items.find((entry) => entry.name === 'manual.pdf')?.path).toBe('docs/manual.pdf');
    const searched = unwrap(store.search('raw-hosted', 'manual.pdf'));
    expect(searched.items[0]).toMatchObject({ name: 'manual.pdf', path: 'docs/manual.pdf' });
  });
});

describe('大仓库分页（NFR-16）', () => {
  it('默认页 50，130 条覆盖 3 页，遍历无重漏', () => {
    const store = createStore('large-repo');
    const first = unwrap(store.listFiles('big-raw', ''));
    expect(first.page_size).toBe(50);
    expect(first.total).toBe(130);
    expect(first.items).toHaveLength(50);
    const seen = new Set<string>();
    for (const page of [1, 2, 3]) {
      for (const entry of unwrap(store.listFiles('big-raw', '', page)).items) {
        seen.add(entry.name);
      }
    }
    expect(unwrap(store.listFiles('big-raw', '', 3)).items).toHaveLength(30);
    expect(seen.size).toBe(130);
  });

  it('page_size 上限 200', () => {
    const store = createStore('large-repo');
    const all = unwrap(store.listFiles('big-raw', '', 1, 1000));
    expect(all.page_size).toBe(200);
    expect(all.items).toHaveLength(130);
  });
});

describe('stress 压测场景（分页/筛选/性能）', () => {
  // 压测场景需构建百万级文件对象，整组共用一次构建，避免逐用例重复重建
  let store: ReturnType<typeof createStore>;
  beforeAll(() => {
    store = createStore('stress');
  });

  it('规模达标：仓库 100、单仓文件 ≥1w、Token 1000、用户 100', () => {
    expect(unwrap(store.listRepos()).total).toBe(100);
    // repo-000 为 raw 格式，1w 文件集中在 dir-0 目录，翻页可见其总数
    expect(unwrap(store.listFiles('repo-000', 'dir-0')).total).toBeGreaterThanOrEqual(10_000);
    expect(unwrap(store.listTokens()).total).toBe(1000);
    expect(unwrap(store.listUsers()).total).toBe(100);
  });

  it('q 名称筛选：仓库/用户/Token 各按子串命中', () => {
    // repo-010..repo-019 共 10 个
    expect(unwrap(store.listRepos(1, 50, 'repo-01')).total).toBe(10);
    // user-010..user-019 共 10 个
    expect(unwrap(store.listUsers(1, 50, 'user-01')).total).toBe(10);
    // token-0010..token-0019 共 10 个
    expect(unwrap(store.listTokens(1, 50, 'token-001')).total).toBe(10);
  });
});

describe('迁移发现与计划', () => {
  it('同名目标仓库产生阻断冲突，改名映射后可创建任务', () => {
    const store = createStore('default');
    const discovery = unwrap(
      store.discover({ kind: 'native-dir', path: 'nexus-data', url: 'https://nexus.example.com' }),
    );
    expect(discovery.repositories.length).toBeGreaterThan(0);
    const blocked = unwrap(
      store.plan({
        discovery_id: discovery.discovery_id,
        mappings: [{ source: 'maven-releases', excluded: false }],
      }),
    );
    expect(blocked.blocked).toBe(true);
    expect(blocked.conflicts.some((c) => c.category === '目标仓库已存在')).toBe(true);
    expect(unwrapError(store.createTask(blocked.plan_id)).status).toBe(400);
    const renamed = unwrap(
      store.plan({
        discovery_id: discovery.discovery_id,
        mappings: [{ source: 'maven-releases', target: 'maven-migrated', excluded: false }],
      }),
    );
    expect(renamed.blocked).toBe(false);
    expect(unwrap(store.createTask(renamed.plan_id)).status).toBe('running');
  });

  it('引用不存在的发现结果返回 400', () => {
    const store = createStore('default');
    const result = store.plan({ discovery_id: 'disc_ghost', mappings: [] });
    expect(unwrapError(result).status).toBe(400);
  });

  it('native-dir 未填 Nexus 地址时 proxy 仓库配置缺失（须手动补），填了则齐全', () => {
    const store = createStore('default');
    const offline = unwrap(store.discover({ kind: 'native-dir', path: 'nexus-data' }));
    const proxyOffline = offline.repositories.find((repo) => repo.repo_type === 'proxy');
    expect(proxyOffline?.config_complete).toBe(false);
    const online = unwrap(
      store.discover({
        kind: 'native-dir',
        path: 'nexus-data',
        url: 'https://nexus.example.com',
        username: 'nexus-admin',
        password: 'nexus-pass',
      }),
    );
    const proxyOnline = online.repositories.find((repo) => repo.repo_type === 'proxy');
    expect(proxyOnline?.config_complete).toBe(true);
  });
});

describe('迁移状态机与进度轮询', () => {
  it('running 任务轮询进度递增', () => {
    const store = createStore('migration-running');
    const before = unwrap(store.getTask('task_run_1')).done_objects;
    const after = unwrap(store.getTask('task_run_1')).done_objects;
    expect(after).toBeGreaterThan(before);
  });

  it('running→pause→paused→resume→running、cancel 链与非法转移 409', () => {
    const store = createStore('migration-running');
    expect(unwrap(store.controlTask('task_run_1', 'pause')).status).toBe('paused');
    expect(unwrap(store.controlTask('task_run_1', 'resume')).status).toBe('running');
    expect(unwrap(store.controlTask('task_run_1', 'cancel')).status).toBe('cancelled');
    expect(unwrap(store.controlTask('task_run_1', 'retry')).status).toBe('running');
    // succeeded 历史任务不允许暂停或继续
    expect(unwrapError(store.controlTask('task_hist_1', 'pause')).status).toBe(409);
    expect(unwrapError(store.controlTask('task_hist_1', 'resume')).status).toBe(409);
  });

  it('partial 场景报告含失败样本与不支持项', () => {
    const store = createStore('migration-partial');
    const report = unwrap(store.getReport('task_part_1'));
    expect(report.status).toBe('partial');
    expect(report.failures).toHaveLength(2);
    expect(report.unsupported.length).toBeGreaterThan(0);
    expect(report.excluded.length).toBeGreaterThan(0);
  });
});

describe('上游失败场景', () => {
  it('proxy 仓库浏览与在线发现返回上游故障错误，hosted 不受影响', () => {
    const store = createStore('upstream-failure');
    expect(unwrapError(store.listFiles('npm-proxy', '')).code).toBe('UPSTREAM_UNAVAILABLE');
    expect(unwrapError(store.search('maven-central', 'lang')).code).toBe('UPSTREAM_UNAVAILABLE');
    const online = store.discover({ kind: 'online', url: 'https://nexus.example.com' });
    expect(unwrapError(online).code).toBe('UPSTREAM_UNAVAILABLE');
    expect(store.listFiles('raw-hosted', '').ok).toBe(true);
    expect(store.isReady()).toBe(false);
  });
});
