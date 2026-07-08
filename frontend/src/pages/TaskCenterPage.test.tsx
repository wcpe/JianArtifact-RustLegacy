// 任务中心页测试（FR-132，仅管理员）：
// 覆盖——活跃/近期任务列表渲染（含 kind/state/label）、无任务空状态、
// 轮询刷新后列表更新（mock setInterval）、后台完成任务仍在列表可找回。
// 注：真浏览器轮询时序 jsdom 难全测，标「待真机」。

import { describe, it, expect, vi, afterEach, beforeEach } from 'vitest';
import { render, screen, waitFor, act, fireEvent } from '@testing-library/react';
import { Button, MantineProvider } from '@mantine/core';
import { MemoryRouter, useLocation } from 'react-router-dom';
import { TaskCenterPage } from './TaskCenterPage';
import * as api from '../api/endpoints';
import type { OnlinePullJob, TaskDetailDto, TaskRecord } from '../api/types';
import { GlobalProgressProvider, useGlobalProgress } from '../hooks/useGlobalProgress';

/** 构造一条任务记录。 */
function 任务(overrides: Partial<TaskRecord> = {}): TaskRecord {
  return {
    id: 'task-1',
    kind: 'migration',
    state: 'running',
    label: '在线拉取迁移',
    started_at: Math.floor(Date.now() / 1000) - 60,
    updated_at: Math.floor(Date.now() / 1000),
    ...overrides,
  };
}

/** 构造一条迁移进度明细。 */
function 迁移进度(overrides: Partial<OnlinePullJob> = {}): OnlinePullJob {
  return {
    job_id: 'task-1',
    phase: 'downloading',
    total_assets: 10,
    done_assets: 3,
    migrated: 2,
    skipped: 1,
    current_repo: 'repo-a',
    current_path: '/content/a.pom',
    paused: false,
    repos: [],
    skipped_repos: [],
    error: null,
    ...overrides,
  };
}

/** 构造一条任务详情。 */
function 任务详情(overrides: Partial<TaskDetailDto> = {}): TaskDetailDto {
  return {
    ...任务(),
    migration: 迁移进度(),
    ...overrides,
  };
}

/** 展示当前路由，便于断言跳转结果。 */
function CurrentLocation() {
  const location = useLocation();
  return <span data-testid="current-location">{`${location.pathname}${location.search}`}</span>;
}

/** 测试页眉刷新按钮的最小消费者。 */
function TestRefreshButton() {
  const { refreshCurrentPage } = useGlobalProgress();
  return <Button onClick={() => void refreshCurrentPage()}>触发页眉刷新</Button>;
}

/** 在 Mantine + Router Provider 下渲染任务中心页。 */
function renderPage(withProgressProvider = false) {
  const content = (
    <MemoryRouter>
      <MantineProvider>
        {withProgressProvider ? (
          <GlobalProgressProvider>
            <TestRefreshButton />
            <TaskCenterPage />
            <CurrentLocation />
          </GlobalProgressProvider>
        ) : (
          <>
            <TaskCenterPage />
            <CurrentLocation />
          </>
        )}
      </MantineProvider>
    </MemoryRouter>
  );
  return render(content);
}

/** 构造可手动完成的 Promise，用于验证请求竞态。 */
function 延迟响应<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

describe('TaskCenterPage', () => {
  beforeEach(() => {
    // 默认桩：空列表，避免触达真实 fetch
    vi.spyOn(api, 'listTasks').mockResolvedValue([]);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('加载后展示 migration 任务（kind 图标 + label + 状态 Badge）', async () => {
    vi.spyOn(api, 'listTasks').mockResolvedValue([
      任务({ kind: 'migration', state: 'running', label: '在线拉取迁移' }),
    ]);
    renderPage();

    await waitFor(() => expect(screen.getByText('在线拉取迁移')).toBeInTheDocument());
    // 状态 Badge 显示「运行中」
    expect(screen.getByText('运行中')).toBeInTheDocument();
  });

  it('展示 update 任务（kind=update，状态 succeeded）', async () => {
    vi.spyOn(api, 'listTasks').mockResolvedValue([
      任务({ id: 'u1', kind: 'update', state: 'succeeded', label: '应用更新' }),
    ]);
    renderPage();

    await waitFor(() => expect(screen.getByText('应用更新')).toBeInTheDocument());
    expect(screen.getByText('已完成')).toBeInTheDocument();
  });

  it('展示 vuln 任务（kind=vuln，状态 failed，error 信息）', async () => {
    vi.spyOn(api, 'listTasks').mockResolvedValue([
      任务({
        id: 'v1',
        kind: 'vuln',
        state: 'failed',
        label: '漏洞库刷新',
        error: '下载超时',
      }),
    ]);
    renderPage();

    await waitFor(() => expect(screen.getByText('漏洞库刷新')).toBeInTheDocument());
    expect(screen.getByText('失败')).toBeInTheDocument();
  });

  it('无任务时显示空状态提示', async () => {
    vi.spyOn(api, 'listTasks').mockResolvedValue([]);
    renderPage();

    await waitFor(() => expect(screen.getByText('暂无任务')).toBeInTheDocument());
  });

  it('轮询后列表更新（新增任务可见）【待真机验证轮询时序】', async () => {
    // 使用 shouldAdvanceTime 让 waitFor 的内部定时器也随之推进
    vi.useFakeTimers({ shouldAdvanceTime: true });

    // 第一次调用：空；第二次调用：有任务
    const spy = vi
      .spyOn(api, 'listTasks')
      .mockResolvedValueOnce([])
      .mockResolvedValue([任务({ label: '轮询新增任务' })]);

    renderPage();

    // 等待首次渲染空状态
    await waitFor(() => expect(screen.getByText('暂无任务')).toBeInTheDocument());

    // 推进 3s（任务中心轮询间隔）触发第二次请求
    await act(async () => {
      vi.advanceTimersByTime(3000);
    });

    await waitFor(() => expect(screen.getByText('轮询新增任务')).toBeInTheDocument());
    expect(spy).toHaveBeenCalledTimes(2);

    vi.useRealTimers();
  });

  it('后台完成的任务仍在列表中（历史保留）', async () => {
    // 后端保留已完成任务，前端直接展示
    vi.spyOn(api, 'listTasks').mockResolvedValue([
      任务({ id: 'done-1', kind: 'migration', state: 'succeeded', label: '已完成迁移' }),
    ]);
    renderPage();

    await waitFor(() => expect(screen.getByText('已完成迁移')).toBeInTheDocument());
    expect(screen.getByText('已完成')).toBeInTheDocument();
  });

  it('同时展示活跃与近期历史任务', async () => {
    vi.spyOn(api, 'listTasks').mockResolvedValue([
      任务({ id: 'r1', kind: 'migration', state: 'running', label: '进行中迁移' }),
      任务({ id: 's1', kind: 'update', state: 'succeeded', label: '已完成更新' }),
    ]);
    renderPage();

    await waitFor(() => expect(screen.getByText('进行中迁移')).toBeInTheDocument());
    expect(screen.getByText('已完成更新')).toBeInTheDocument();
  });

  it('点击任务行展开详情而不是直接跳走', async () => {
    vi.spyOn(api, 'listTasks').mockResolvedValue([
      任务({ id: 'task-1', kind: 'migration', state: 'running', label: '离线预览任务' }),
    ]);
    const detailSpy = vi.spyOn(api, 'getTask').mockResolvedValue(
      任务详情({
        id: 'task-1',
        kind: 'migration',
        state: 'running',
        label: '离线预览任务',
        migration: 迁移进度({ current_repo: 'repo-a' }),
      }),
    );
    renderPage();

    await waitFor(() => expect(screen.getByText('离线预览任务')).toBeInTheDocument());
    fireEvent.click(screen.getByText('离线预览任务'));

    await waitFor(() => expect(detailSpy).toHaveBeenCalledWith('task-1'));
    expect(screen.getByText('任务详情')).toBeInTheDocument();
    expect(screen.getByText('repo-a')).toBeInTheDocument();
  });

  it('点击同一任务两次可收起详情', async () => {
    vi.spyOn(api, 'listTasks').mockResolvedValue([
      任务({ id: 'task-1', kind: 'migration', state: 'running', label: '可收起迁移' }),
    ]);
    const detailSpy = vi.spyOn(api, 'getTask').mockResolvedValue(
      任务详情({
        id: 'task-1',
        label: '可收起迁移',
        migration: 迁移进度({ current_repo: 'repo-a' }),
      }),
    );
    renderPage();

    await waitFor(() => expect(screen.getByText('可收起迁移')).toBeInTheDocument());
    fireEvent.click(screen.getByText('可收起迁移'));
    await waitFor(() => expect(screen.getByText('任务详情')).toBeInTheDocument());

    fireEvent.click(screen.getByText('可收起迁移'));

    expect(screen.queryByText('任务详情')).not.toBeInTheDocument();
    expect(screen.queryByText('repo-a')).not.toBeInTheDocument();
    expect(detailSpy).toHaveBeenCalledTimes(1);
  });

  it('快速切换任务时旧详情响应不会覆盖新详情', async () => {
    vi.spyOn(api, 'listTasks').mockResolvedValue([
      任务({ id: 'task-1', kind: 'migration', state: 'running', label: '旧任务' }),
      任务({ id: 'task-2', kind: 'migration', state: 'running', label: '新任务' }),
    ]);
    const 旧响应 = 延迟响应<TaskDetailDto>();
    const 新响应 = 延迟响应<TaskDetailDto>();
    vi.spyOn(api, 'getTask').mockImplementation((id) => {
      if (id === 'task-1') return 旧响应.promise;
      return 新响应.promise;
    });
    renderPage();

    await waitFor(() => expect(screen.getByText('旧任务')).toBeInTheDocument());
    fireEvent.click(screen.getByText('旧任务'));
    fireEvent.click(screen.getByText('新任务'));

    await act(async () => {
      新响应.resolve(
        任务详情({
          id: 'task-2',
          label: '新任务',
          migration: 迁移进度({ job_id: 'task-2', current_repo: 'repo-new' }),
        }),
      );
    });
    await waitFor(() => expect(screen.getByText('repo-new')).toBeInTheDocument());

    await act(async () => {
      旧响应.resolve(
        任务详情({
          id: 'task-1',
          label: '旧任务',
          migration: 迁移进度({ job_id: 'task-1', current_repo: 'repo-old' }),
        }),
      );
    });

    expect(screen.getByText('repo-new')).toBeInTheDocument();
    expect(screen.queryByText('repo-old')).not.toBeInTheDocument();
  });

  it('离线预览任务详情返回后显示预览摘要而不是搬运计数', async () => {
    vi.spyOn(api, 'listTasks').mockResolvedValue([
      任务({ id: 'preview-1', kind: 'migration', state: 'succeeded', label: '离线预览' }),
    ]);
    const 详情响应 = 延迟响应<TaskDetailDto>();
    vi.spyOn(api, 'getTask').mockReturnValue(详情响应.promise);
    renderPage();

    await waitFor(() => expect(screen.getByText('离线预览')).toBeInTheDocument());
    fireEvent.click(screen.getByText('离线预览'));
    expect(screen.getByText(/正在加载任务详情/)).toBeInTheDocument();

    await act(async () => {
      详情响应.resolve(
        任务详情({
          id: 'preview-1',
          kind: 'migration',
          state: 'succeeded',
          label: '离线预览',
          migration: 迁移进度({
            job_id: 'preview-1',
            phase: 'done',
            total_assets: 4,
            done_assets: 4,
            migrated: 0,
            skipped: 0,
            current_repo: null,
            current_path: null,
            offline_preview: [{ repo_name: 'raw-hosted', blob_count: 4, blobs: [] }],
          }),
        }),
      );
    });

    await waitFor(() => expect(screen.queryByText(/正在加载任务详情/)).not.toBeInTheDocument());
    expect(screen.getByText('任务详情')).toBeInTheDocument();
    expect(screen.getByText('ID：preview-1')).toBeInTheDocument();
    expect(screen.getByText(/进度 4 \/ 4/)).toBeInTheDocument();
    expect(screen.getByText('离线预览已完成：共 1 个仓库、4 个 blob。')).toBeInTheDocument();
    expect(screen.getByText('raw-hosted：4 个 blob')).toBeInTheDocument();
    expect(screen.queryByText('已迁 0')).not.toBeInTheDocument();
    expect(screen.queryByText('已跳过 0')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: '暂停' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: '继续' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: '结束' })).not.toBeInTheDocument();
  });

  it('迁移任务详情提供暂停和结束控制', async () => {
    vi.spyOn(api, 'listTasks').mockResolvedValue([
      任务({ id: 'task-1', kind: 'migration', state: 'running', label: '可控迁移' }),
    ]);
    vi.spyOn(api, 'getTask').mockResolvedValue(
      任务详情({ id: 'task-1', label: '可控迁移', migration: 迁移进度() }),
    );
    const pauseSpy = vi.spyOn(api, 'pauseMigrationJob').mockResolvedValue();
    const cancelSpy = vi.spyOn(api, 'cancelMigrationJob').mockResolvedValue();
    renderPage();

    await waitFor(() => expect(screen.getByText('可控迁移')).toBeInTheDocument());
    fireEvent.click(screen.getByText('可控迁移'));
    await waitFor(() => expect(screen.getByText('任务详情')).toBeInTheDocument());

    fireEvent.click(screen.getByRole('button', { name: '暂停' }));
    await waitFor(() => expect(pauseSpy).toHaveBeenCalledWith('task-1'));

    fireEvent.click(screen.getByRole('button', { name: '结束' }));
    await waitFor(() => expect(cancelSpy).toHaveBeenCalledWith('task-1'));
  });

  it('任务中心：离线搬运详情展示当前文件，跳转查看携带同一 job_id', async () => {
    const 当前文件 = 'pool/vol-01/chap-01/com/example/demo/1.0/demo-1.0.jar';
    vi.spyOn(api, 'listTasks').mockResolvedValue([
      任务({ id: 'off-live', kind: 'migration', state: 'running', label: '离线 proxy 搬运' }),
    ]);
    vi.spyOn(api, 'getTask').mockResolvedValue(
      任务详情({
        id: 'off-live',
        kind: 'migration',
        state: 'running',
        label: '离线 proxy 搬运',
        migration: 迁移进度({
          job_id: 'off-live',
          current_repo: 'raw-hosted',
          current_path: 当前文件,
        }),
      }),
    );
    renderPage();

    await waitFor(() => expect(screen.getByText('离线 proxy 搬运')).toBeInTheDocument());
    fireEvent.click(screen.getByText('离线 proxy 搬运'));

    await waitFor(() => expect(screen.getByText('任务详情')).toBeInTheDocument());
    expect(screen.getByText('raw-hosted')).toBeInTheDocument();
    expect(screen.getByText(`当前文件：${当前文件}`)).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: '跳转查看' }));

    expect(screen.getByTestId('current-location')).toHaveTextContent('/migration?job_id=off-live');
  });

  it('迁移任务详情跳转查看携带 job_id', async () => {
    vi.spyOn(api, 'listTasks').mockResolvedValue([
      任务({ id: 'mig-1', kind: 'migration', state: 'succeeded', label: '已完成迁移' }),
    ]);
    vi.spyOn(api, 'getTask').mockResolvedValue(
      任务详情({ id: 'mig-1', state: 'succeeded', label: '已完成迁移', migration: 迁移进度() }),
    );
    renderPage();

    await waitFor(() => expect(screen.getByText('已完成迁移')).toBeInTheDocument());
    fireEvent.click(screen.getByText('已完成迁移'));
    await waitFor(() => expect(screen.getByText('任务详情')).toBeInTheDocument());
    fireEvent.click(screen.getByRole('button', { name: '跳转查看' }));

    expect(screen.getByTestId('current-location')).toHaveTextContent('/migration?job_id=mig-1');
  });

  it('已注册页眉刷新回调时可刷新任务列表', async () => {
    const listSpy = vi
      .spyOn(api, 'listTasks')
      .mockResolvedValueOnce([])
      .mockResolvedValueOnce([任务({ id: 'refresh-1', label: '刷新出现的任务' })]);
    renderPage(true);

    await waitFor(() => expect(screen.getByText('暂无任务')).toBeInTheDocument());
    fireEvent.click(screen.getByRole('button', { name: '触发页眉刷新' }));

    await waitFor(() => expect(screen.getByText('刷新出现的任务')).toBeInTheDocument());
    expect(listSpy).toHaveBeenCalledTimes(2);
  });

  it('已暂停迁移任务详情提供继续控制', async () => {
    vi.spyOn(api, 'listTasks').mockResolvedValue([
      任务({ id: 'task-1', kind: 'migration', state: 'paused', label: '暂停迁移' }),
    ]);
    vi.spyOn(api, 'getTask').mockResolvedValue(
      任务详情({
        id: 'task-1',
        state: 'paused',
        label: '暂停迁移',
        migration: 迁移进度({ phase: 'paused', paused: true }),
      }),
    );
    const resumeSpy = vi.spyOn(api, 'resumeMigrationJob').mockResolvedValue();
    renderPage();

    await waitFor(() => expect(screen.getByText('暂停迁移')).toBeInTheDocument());
    fireEvent.click(screen.getByText('暂停迁移'));
    await waitFor(() => expect(screen.getByText('任务详情')).toBeInTheDocument());

    fireEvent.click(screen.getByRole('button', { name: '继续' }));
    await waitFor(() => expect(resumeSpy).toHaveBeenCalledWith('task-1'));
  });
});
