// 数据模型与场景行为测试（M1-FR-035）：会话、Token、用户保护、仓库权限、分页与迁移状态机。
import { describe, expect, it } from 'vitest';
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

  it('错误凭据与未知用户登录均返回 401', () => {
    const store = createStore('default');
    store.logout();
    expect(unwrapError(store.login('admin', '错误口令')).status).toBe(401);
    expect(unwrapError(store.login('ghost', 'x')).status).toBe(401);
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
    expect(unwrapError(store.revokeToken(created.id)).status).toBe(404);
  });
});

describe('用户管理', () => {
  it('最后一名启用管理员不可禁用或降级；补充管理员后可禁用', () => {
    const store = createStore('default');
    expect(unwrapError(store.updateUser(1, { enabled: false })).status).toBe(403);
    expect(unwrapError(store.updateUser(1, { role: 'user' })).status).toBe(403);
    unwrap(store.createUser({ username: 'admin2', password: 'admin2pass', role: 'admin' }));
    expect(store.updateUser(1, { enabled: false }).ok).toBe(true);
  });

  it('restricted 场景下普通用户访问用户管理被拒（403）', () => {
    const store = createStore('restricted');
    expect(unwrapError(store.listUsers()).status).toBe(403);
    const created = store.createUser({ username: 'x', password: 'p', role: 'user' });
    expect(unwrapError(created).status).toBe(403);
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

describe('迁移发现与计划', () => {
  it('同名目标仓库产生阻断冲突，改名映射后可创建任务', () => {
    const store = createStore('default');
    const discovery = unwrap(
      store.discover({ kind: 'native-dir', path: 'nexus-data', config_source: 'rest' }),
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
