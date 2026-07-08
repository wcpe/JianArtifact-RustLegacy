// Nexus 迁移管理页面组件测试（FR-81）：
// 在线 / 离线两形态预览 → 勾选 → 执行搬运 → 展示报告；凭据引用不回显明文；错误展示文案。

import { describe, it, expect, vi, afterEach, beforeEach } from 'vitest';
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MantineProvider } from '@mantine/core';
import { Notifications } from '@mantine/notifications';
import { MemoryRouter } from 'react-router-dom';
import { MigrationPage } from './MigrationPage';
import * as api from '../api/endpoints';
import { ApiError } from '../api/client';
import type {
  NexusRepoSummary,
  OfflineRepoSummary,
  OnlinePullJob,
  TaskDetailDto,
  TaskRecord,
} from '../api/types';

/** 在 Provider 下渲染迁移页（含通知容器）。 */
function renderPage(initialEntry = '/migration') {
  return render(
    <MemoryRouter initialEntries={[initialEntry]}>
      <MantineProvider>
        <Notifications />
        <MigrationPage />
      </MantineProvider>
    </MemoryRouter>,
  );
}

const 在线仓库: NexusRepoSummary[] = [
  { name: 'maven-proxy', format: 'maven2', type: 'proxy', upstream_url: 'https://repo1.example' },
  { name: 'npm-hosted', format: 'npm', type: 'hosted', upstream_url: null },
];

const 在线仓库含maven: NexusRepoSummary[] = [
  { name: 'maven-releases', format: 'maven2', type: 'hosted', upstream_url: null },
  { name: 'npm-hosted', format: 'npm', type: 'hosted', upstream_url: null },
];

/** 任务进行中快照（downloading）：进度过半、含当前仓库 / 当前文件。 */
const 任务进行中: OnlinePullJob = {
  job_id: 'job-1',
  phase: 'downloading',
  total_assets: 7,
  done_assets: 3,
  migrated: 3,
  skipped: 0,
  current_repo: 'maven-releases',
  current_path: 'com/example/app/1.0/app-1.0.jar',
  paused: false,
  repos: [],
  skipped_repos: [],
  error: null,
};

/** 任务暂停态快照（paused）：用于断言「继续」按钮可用、「暂停」按钮被「继续」替换。 */
const 任务已暂停: OnlinePullJob = {
  ...任务进行中,
  phase: 'paused',
  paused: true,
};

/** 任务已取消终态快照（cancelled）：不算失败、保留已搬运。 */
const 任务已取消: OnlinePullJob = {
  ...任务进行中,
  phase: 'cancelled',
  paused: false,
};

/** 任务终态快照（done）：含每仓库明细与整仓跳过列表。 */
const 任务完成: OnlinePullJob = {
  job_id: 'job-1',
  phase: 'done',
  total_assets: 7,
  done_assets: 7,
  migrated: 7,
  skipped: 0,
  current_repo: null,
  current_path: null,
  paused: false,
  repos: [
    {
      source_repo: 'maven-releases',
      target_repo: 'maven-mirror',
      format: 'maven',
      created: true,
      migrated_artifacts: 7,
      skipped_artifacts: 0,
    },
  ],
  skipped_repos: ['npm-hosted'],
  error: null,
};

const 离线仓库: OfflineRepoSummary[] = [
  {
    repo_name: 'raw-hosted',
    blob_count: 2,
    blobs: [
      { blob_name: 'a/b.txt', sha1: 'abc', size: 12 },
      { blob_name: 'c/d.txt', sha1: null, size: null },
    ],
  },
];

const 活跃迁移任务: TaskRecord = {
  id: 'job-active',
  kind: 'migration',
  state: 'running',
  label: '在线拉取迁移',
  started_at: 1,
  updated_at: 2,
};

const 离线当前文件 = 'pool/vol-01/chap-01/com/example/demo/1.0/demo-1.0.jar';

const 离线搬运进行中: OnlinePullJob = {
  job_id: 'off-live',
  phase: 'downloading',
  total_assets: 2,
  done_assets: 1,
  migrated: 1,
  skipped: 0,
  current_repo: 'raw-hosted',
  current_path: 离线当前文件,
  paused: false,
  repos: [],
  skipped_repos: [],
  error: null,
};

const 离线搬运完成: OnlinePullJob = {
  ...离线搬运进行中,
  phase: 'done',
  done_assets: 2,
  migrated: 2,
  current_repo: null,
  current_path: null,
  repos: [
    {
      source_repo: 'raw-hosted',
      target_repo: 'raw-hosted',
      format: 'raw',
      created: false,
      migrated_artifacts: 2,
      skipped_artifacts: 0,
      skipped_existing_artifacts: 0,
    },
  ],
};

function 任务详情(job: OnlinePullJob, label: string): TaskDetailDto {
  return {
    id: job.job_id,
    kind: 'migration',
    state: job.phase === 'done' ? 'succeeded' : 'running',
    label,
    started_at: 1,
    updated_at: 2,
    finished_at: job.phase === 'done' ? 3 : undefined,
    migration: job,
  };
}

describe('MigrationPage', () => {
  beforeEach(() => {
    vi.spyOn(api, 'listTasks').mockResolvedValue([]);
    vi.spyOn(api, 'getTask').mockRejectedValue(new ApiError(404, 'not_found', '任务不存在'));
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.useRealTimers();
    // 清理在线任务存档，避免跨用例触发重连副作用。
    localStorage.clear();
  });

  it('在线预览：填源地址后枚举仓库并展示列表', async () => {
    const spy = vi.spyOn(api, 'previewNexusRepositories').mockResolvedValue(在线仓库);
    const user = userEvent.setup();
    renderPage();

    await user.type(screen.getByPlaceholderText('https://nexus.example'), 'https://nexus.example');
    await user.click(screen.getByRole('button', { name: '预览仓库' }));

    await waitFor(() => expect(screen.getByText('maven-proxy')).toBeInTheDocument());
    expect(screen.getByText('npm-hosted')).toBeInTheDocument();
    // 仅引用、不回显明文：auth_ref 字段为口令型输入
    expect(spy).toHaveBeenCalledWith({ base_url: 'https://nexus.example', auth_ref: undefined });
  });

  it('凭据引用输入框为口令型（不回显明文）', async () => {
    renderPage();
    const input = screen.getByPlaceholderText('例如 NEXUS_SRC');
    expect(input).toHaveAttribute('type', 'password');
  });

  it('离线预览：异步任务通过任务详情摘要回填可勾选列表（FR-124）', async () => {
    // FR-124：离线预览改异步，前端只需要仓库名与 blob 数，应走轻量任务详情摘要避免拉取巨大 blob 明细。
    const spy = vi.spyOn(api, 'previewNexusOffline').mockResolvedValue({ job_id: 'prev-1' });
    const taskSpy = vi.spyOn(api, 'getTask').mockResolvedValue({
      id: 'prev-1',
      kind: 'migration',
      state: 'succeeded',
      label: '离线预览',
      started_at: 1,
      updated_at: 2,
      migration: {
        ...任务完成,
        job_id: 'prev-1',
        phase: 'done',
        total_assets: 2,
        done_assets: 2,
        migrated: 0,
        skipped: 0,
        repos: [],
        skipped_repos: [],
        offline_preview: 离线仓库.map((repo) => ({ ...repo, blobs: [] })),
      },
    });
    const jobSpy = vi
      .spyOn(api, 'getMigrationJob')
      .mockRejectedValue(new Error('不应直接拉取离线预览大明细'));
    const user = userEvent.setup();
    renderPage();

    await user.click(screen.getByRole('radio', { name: /离线/ }));
    expect(screen.queryByPlaceholderText('https://nexus.example')).not.toBeInTheDocument();
    expect(screen.queryByPlaceholderText('例如 NEXUS_SRC')).not.toBeInTheDocument();
    await user.type(screen.getByPlaceholderText('/data/nexus/blobs/default'), '/data/blobs');

    await user.click(screen.getByRole('button', { name: '预览仓库' }));

    await waitFor(() => expect(screen.getByText('raw-hosted')).toBeInTheDocument());
    expect(screen.getByText('2 个 blob')).toBeInTheDocument();
    expect(screen.queryByText('无仓库被搬运。')).not.toBeInTheDocument();
    // 提交预览只带路径；枚举结果经任务详情摘要轮询取回，不走会返回大明细的专用进度端点。
    expect(spy).toHaveBeenCalledWith({ path: '/data/blobs' });
    expect(taskSpy).toHaveBeenCalledWith('prev-1');
    expect(jobSpy).not.toHaveBeenCalledWith('prev-1');

    await user.click(screen.getByRole('button', { name: '下一步：勾选执行' }));
    expect(screen.getByRole('checkbox', { name: /raw-hosted/ })).toBeInTheDocument();
  });

  it('离线预览：任务详情不可用时兜底迁移进度端点，避免预览结果丢失', async () => {
    vi.spyOn(api, 'previewNexusOffline').mockResolvedValue({ job_id: 'prev-fallback' });
    const taskSpy = vi
      .spyOn(api, 'getTask')
      .mockRejectedValue(new ApiError(404, 'not_found', '任务不存在'));
    const jobSpy = vi.spyOn(api, 'getMigrationJob').mockResolvedValue({
      ...任务完成,
      job_id: 'prev-fallback',
      total_assets: 2,
      done_assets: 2,
      migrated: 0,
      skipped: 0,
      repos: [],
      skipped_repos: [],
      offline_preview: 离线仓库,
    });
    const user = userEvent.setup();
    renderPage();

    await user.click(screen.getByRole('radio', { name: /离线/ }));
    await user.type(screen.getByPlaceholderText('/data/nexus/blobs/default'), '/data/blobs');
    await user.click(screen.getByRole('button', { name: '预览仓库' }));

    await waitFor(() => expect(screen.getByText('raw-hosted')).toBeInTheDocument());
    expect(taskSpy).toHaveBeenCalledWith('prev-fallback');
    expect(jobSpy).toHaveBeenCalledWith('prev-fallback');
  });

  it('离线预览：后端缺失 job_id 时不请求 undefined 任务并展示明确错误', async () => {
    vi.spyOn(api, 'previewNexusOffline').mockResolvedValue({} as never);
    const taskSpy = vi.spyOn(api, 'getTask').mockResolvedValue({} as never);
    const jobSpy = vi.spyOn(api, 'getMigrationJob').mockResolvedValue(任务完成);
    const user = userEvent.setup();
    renderPage();

    await user.click(screen.getByRole('radio', { name: /离线/ }));
    await user.type(screen.getByPlaceholderText('/data/nexus/blobs/default'), '/data/blobs');
    await user.click(screen.getByRole('button', { name: '预览仓库' }));

    await waitFor(() =>
      expect(screen.getByText('后端未返回任务 ID，请稍后重试')).toBeInTheDocument(),
    );
    expect(taskSpy).not.toHaveBeenCalled();
    expect(jobSpy).not.toHaveBeenCalled();
  });

  it('客户端重连：通过 job_id 恢复离线预览时展示 offline_preview 结果', async () => {
    const jobSpy = vi.spyOn(api, 'getMigrationJob').mockResolvedValue({
      ...任务完成,
      job_id: 'prev-restore',
      total_assets: 2,
      done_assets: 2,
      migrated: 0,
      skipped: 0,
      repos: [],
      skipped_repos: [],
      offline_preview: 离线仓库,
    });
    vi.spyOn(api, 'getTask').mockRejectedValue(new Error('任务登记表暂不可用'));

    renderPage('/migration?job_id=prev-restore');

    await waitFor(() => expect(jobSpy).toHaveBeenCalledWith('prev-restore'));
    await waitFor(() => expect(screen.getByText('raw-hosted')).toBeInTheDocument());
    expect(screen.getByText('离线预览已完成：共 1 个仓库、2 个 blob。')).toBeInTheDocument();
  });

  it('执行 proxy 离线搬运（异步）：离线入口不经过在线拉取，发起任务后轮询至 done 展示报告（FR-125）', async () => {
    vi.spyOn(api, 'previewNexusOffline').mockResolvedValue({ job_id: 'prev-offline' });
    vi.spyOn(api, 'getTask').mockImplementation(async (id: string) => {
      if (id !== 'prev-offline') {
        throw new ApiError(404, 'not_found', '任务不存在');
      }
      return {
        id: 'prev-offline',
        kind: 'migration',
        state: 'succeeded',
        label: '离线预览',
        started_at: 1,
        updated_at: 2,
        migration: {
          ...任务完成,
          job_id: 'prev-offline',
          phase: 'done',
          total_assets: 2,
          done_assets: 2,
          migrated: 0,
          skipped: 0,
          repos: [],
          skipped_repos: [],
          offline_preview: [{ repo_name: 'maven-proxy', blob_count: 2, blobs: [] }],
        },
      };
    });
    const migrateSpy = vi.spyOn(api, 'migrateNexusProxy').mockResolvedValue({ job_id: 'off-1' });
    const onlineSpy = vi
      .spyOn(api, 'migrateNexusOnline')
      .mockResolvedValue({ job_id: 'online-wrong' });
    const jobSpy = vi.spyOn(api, 'getMigrationJob').mockResolvedValue({
      ...任务完成,
      total_assets: 10,
      done_assets: 10,
      migrated: 7,
      skipped_existing: 2,
      skipped: 1,
      repos: [
        {
          ...任务完成.repos[0],
          migrated_artifacts: 7,
          skipped_existing_artifacts: 2,
          skipped_artifacts: 1,
        },
      ],
    });
    const user = userEvent.setup();
    renderPage();

    await user.click(screen.getByRole('radio', { name: /离线/ }));
    expect(screen.queryByPlaceholderText('https://nexus.example')).not.toBeInTheDocument();
    expect(screen.queryByPlaceholderText('例如 NEXUS_SRC')).not.toBeInTheDocument();
    await user.type(screen.getByPlaceholderText('/data/nexus/blobs/default'), '/data/blobs');
    await user.click(screen.getByRole('button', { name: '预览仓库' }));
    await waitFor(() => expect(screen.getByText('maven-proxy')).toBeInTheDocument());

    await user.click(screen.getByRole('button', { name: '下一步：勾选执行' }));
    expect(screen.queryByRole('button', { name: '执行在线拉取' })).not.toBeInTheDocument();
    await user.click(screen.getByRole('checkbox', { name: /maven-proxy/ }));
    expect(screen.getByPlaceholderText('/data/nexus/blobs/default')).toHaveValue('/data/blobs');
    expect(screen.getByRole('button', { name: '执行 proxy 搬运' })).toBeEnabled();
    expect(screen.getByRole('button', { name: '执行 hosted 搬运' })).toBeEnabled();
    await user.click(screen.getByRole('button', { name: '执行 proxy 搬运' }));

    await waitFor(() => expect(migrateSpy).toHaveBeenCalled());
    expect(onlineSpy).not.toHaveBeenCalled();
    expect(migrateSpy).toHaveBeenCalledWith({
      offline_path: '/data/blobs',
      repositories: ['maven-proxy'],
    });
    await waitFor(() => expect(jobSpy).toHaveBeenCalledWith('off-1'));
    await waitFor(() => expect(screen.getByText('已完成')).toBeInTheDocument(), { timeout: 3000 });
    expect(screen.getByText('总数 10')).toBeInTheDocument();
    expect(screen.getAllByText('新搬 7').length).toBeGreaterThan(0);
    expect(screen.getAllByText('已存在跳过 2').length).toBeGreaterThan(0);
    expect(screen.getAllByText('失败跳过 1').length).toBeGreaterThan(0);
    const reportTable = screen.getByText('maven-releases', { selector: 'td' }).closest('table')!;
    expect(within(reportTable).getByText('7')).toBeInTheDocument();
    expect(within(reportTable).getByText('2')).toBeInTheDocument();
    expect(within(reportTable).getByText('1')).toBeInTheDocument();
  });

  it('离线完整流程：预览后勾选仓库，开始搬运会自动进入进度页并显示当前文件', async () => {
    vi.spyOn(api, 'previewNexusOffline').mockResolvedValue({ job_id: 'prev-live' });
    let 搬运读取次数 = 0;
    vi.spyOn(api, 'getTask').mockImplementation(async (id: string) => {
      if (id === 'prev-live') {
        return 任务详情(
          {
            ...任务完成,
            job_id: 'prev-live',
            phase: 'done',
            total_assets: 2,
            done_assets: 2,
            migrated: 0,
            skipped: 0,
            repos: [],
            skipped_repos: [],
            offline_preview: 离线仓库.map((repo) => ({ ...repo, blobs: [] })),
          },
          '离线预览',
        );
      }
      if (id === 'off-live') {
        搬运读取次数 += 1;
        return 任务详情(搬运读取次数 === 1 ? 离线搬运进行中 : 离线搬运完成, '离线 proxy 搬运');
      }
      throw new ApiError(404, 'not_found', '任务不存在');
    });
    const migrateSpy = vi.spyOn(api, 'migrateNexusProxy').mockResolvedValue({ job_id: 'off-live' });
    const user = userEvent.setup();
    renderPage();

    await user.click(screen.getByRole('radio', { name: /离线/ }));
    await user.type(screen.getByPlaceholderText('/data/nexus/blobs/default'), '/data/blobs');
    await user.click(screen.getByRole('button', { name: '预览仓库' }));
    await waitFor(() => expect(screen.getByText('raw-hosted')).toBeInTheDocument());
    await user.click(screen.getByRole('button', { name: '下一步：勾选执行' }));

    const proxyButton = screen.getByRole('button', { name: '执行 proxy 搬运' });
    expect(proxyButton).toBeDisabled();
    await user.click(screen.getByRole('checkbox', { name: /raw-hosted/ }));
    expect(proxyButton).toBeEnabled();
    await user.click(proxyButton);

    await waitFor(() =>
      expect(migrateSpy).toHaveBeenCalledWith({
        offline_path: '/data/blobs',
        repositories: ['raw-hosted'],
      }),
    );
    await waitFor(() => expect(screen.getByText('离线 proxy 搬运')).toBeInTheDocument());
    expect(screen.getByRole('button', { name: '返回勾选' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: '执行 proxy 搬运' })).not.toBeInTheDocument();
    await waitFor(() => expect(screen.getByText('下载搬运中')).toBeInTheDocument());
    expect(screen.getByText(/进度 1 \/ 2/)).toBeInTheDocument();
    expect(screen.getByText(/当前仓库：/)).toHaveTextContent('raw-hosted');
    expect(
      screen.getByText(new RegExp(离线当前文件.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'))),
    ).toBeInTheDocument();

    await waitFor(() => expect(screen.getByText('已完成')).toBeInTheDocument(), { timeout: 3000 });
    expect(screen.getByText('总数 2')).toBeInTheDocument();
    const repoCells = screen.getAllByText('raw-hosted', { selector: 'td' });
    expect(repoCells.length).toBeGreaterThanOrEqual(2);
    const reportTable = repoCells[0].closest('table')!;
    expect(within(reportTable).getAllByText('2').length).toBeGreaterThan(0);
  });

  it('客户端重连：离线搬运 job_id 重新进入时展示同一任务详情与当前文件', async () => {
    const taskSpy = vi
      .spyOn(api, 'getTask')
      .mockResolvedValue(任务详情(离线搬运进行中, '离线 proxy 搬运'));

    const firstRender = renderPage('/migration?job_id=off-live');

    await waitFor(() => expect(taskSpy).toHaveBeenCalledWith('off-live'));
    await waitFor(() => expect(screen.getByText('离线 proxy 搬运')).toBeInTheDocument());
    expect(screen.getByText('下载搬运中')).toBeInTheDocument();
    expect(screen.getByText(/当前仓库：/)).toHaveTextContent('raw-hosted');
    expect(
      screen.getByText(new RegExp(离线当前文件.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'))),
    ).toBeInTheDocument();

    taskSpy.mockClear();
    firstRender.unmount();
    renderPage('/migration?job_id=off-live');

    await waitFor(() => expect(taskSpy).toHaveBeenCalledWith('off-live'));
    await waitFor(() => expect(screen.getByText('离线 proxy 搬运')).toBeInTheDocument());
    expect(screen.getByText('下载搬运中')).toBeInTheDocument();
    expect(screen.getByText(/当前仓库：/)).toHaveTextContent('raw-hosted');
    expect(
      screen.getByText(new RegExp(离线当前文件.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'))),
    ).toBeInTheDocument();
  });

  it('离线入口：不显示源地址或凭据，未填源地址也能直接发起 proxy 搬运', async () => {
    vi.spyOn(api, 'previewNexusOffline').mockResolvedValue({ job_id: 'prev-no-base' });
    vi.spyOn(api, 'getTask').mockResolvedValue({
      id: 'prev-no-base',
      kind: 'migration',
      state: 'succeeded',
      label: '离线预览',
      started_at: 1,
      updated_at: 2,
      migration: {
        ...任务完成,
        job_id: 'prev-no-base',
        phase: 'done',
        total_assets: 1,
        done_assets: 1,
        migrated: 0,
        skipped: 0,
        repos: [],
        skipped_repos: [],
        offline_preview: [{ repo_name: 'raw-hosted', blob_count: 1, blobs: [] }],
      },
    });
    const migrateSpy = vi
      .spyOn(api, 'migrateNexusProxy')
      .mockResolvedValue({ job_id: 'off-no-base' });
    const user = userEvent.setup();
    renderPage();

    await user.click(screen.getByRole('radio', { name: /离线/ }));
    expect(screen.queryByPlaceholderText('https://nexus.example')).not.toBeInTheDocument();
    expect(screen.queryByPlaceholderText('例如 NEXUS_SRC')).not.toBeInTheDocument();
    await user.type(screen.getByPlaceholderText('/data/nexus/blobs/default'), '/data/blobs');
    await user.click(screen.getByRole('button', { name: '预览仓库' }));
    await waitFor(() => expect(screen.getByText('raw-hosted')).toBeInTheDocument());
    await user.click(screen.getByRole('button', { name: '下一步：勾选执行' }));
    await user.click(screen.getByRole('checkbox', { name: /raw-hosted/ }));
    expect(screen.getByPlaceholderText('/data/nexus/blobs/default')).toHaveValue('/data/blobs');

    const proxyButton = screen.getByRole('button', { name: '执行 proxy 搬运' });
    const hostedButton = screen.getByRole('button', { name: '执行 hosted 搬运' });
    expect(proxyButton).toBeEnabled();
    expect(hostedButton).toBeEnabled();
    expect(screen.queryByRole('button', { name: '执行在线拉取' })).not.toBeInTheDocument();

    await user.click(proxyButton);

    await waitFor(() => expect(migrateSpy).toHaveBeenCalled());
    expect(migrateSpy).toHaveBeenCalledWith({
      offline_path: '/data/blobs',
      repositories: ['raw-hosted'],
    });
    expect(screen.queryByText(/离线搬运需要源 Nexus 地址/)).not.toBeInTheDocument();
  });

  it('在线拉取：发起异步任务后轮询进度，downloading→done 渲染队列进度与最终报告', async () => {
    vi.spyOn(api, 'previewNexusRepositories').mockResolvedValue(在线仓库含maven);
    const onlineSpy = vi.spyOn(api, 'migrateNexusOnline').mockResolvedValue({ job_id: 'job-1' });
    // 任务进度顺序：首次返回 downloading，下一周期返回 done。
    const jobSpy = vi
      .spyOn(api, 'getMigrationJob')
      .mockResolvedValueOnce(任务进行中)
      .mockResolvedValue(任务完成);
    const user = userEvent.setup();
    renderPage();

    await user.type(screen.getByPlaceholderText('https://nexus.example'), 'https://nexus.example');
    await user.click(screen.getByRole('button', { name: '预览仓库' }));
    await waitFor(() => expect(screen.getByText('maven-releases')).toBeInTheDocument());

    // 进入勾选步骤（默认迁移方式即为「在线拉取」）
    await user.click(screen.getByRole('button', { name: '下一步：勾选执行' }));
    // 勾选 maven 仓库并填目标改名
    await user.click(screen.getByRole('checkbox', { name: /maven-releases/ }));
    await user.type(
      screen.getByRole('textbox', { name: 'maven-releases 目标仓库名' }),
      'maven-mirror',
    );
    // 发起在线拉取（异步，立即返回 job_id）
    await user.click(screen.getByRole('button', { name: '执行在线拉取' }));

    await waitFor(() => expect(onlineSpy).toHaveBeenCalled());
    // 仅发起所选仓库（含改名），未选仓库不出现在请求里
    expect(onlineSpy).toHaveBeenCalledWith({
      base_url: 'https://nexus.example',
      auth_ref: undefined,
      repositories: [{ source: 'maven-releases', target: 'maven-mirror' }],
    });
    // job_id 存档供重连
    expect(localStorage.getItem('jian.migrate.online.jobId')).toBe('job-1');

    // 首次轮询（发起后立即拉一次）：展示进行中队列（当前仓库 / 当前文件 / 进度计数）
    await waitFor(() => expect(jobSpy).toHaveBeenCalledWith('job-1'));
    await waitFor(() => expect(screen.getByText('下载搬运中')).toBeInTheDocument());
    expect(screen.getByText(/com\/example\/app\/1\.0\/app-1\.0\.jar/)).toBeInTheDocument();
    expect(screen.getByText(/进度 3 \/ 7/)).toBeInTheDocument();

    // 经下一个轮询周期（约 1.5s）转入终态 done，展示最终报告
    await waitFor(() => expect(screen.getByText('已完成')).toBeInTheDocument(), { timeout: 3000 });
    const reportTable = screen.getByText('maven-releases', { selector: 'td' }).closest('table')!;
    expect(within(reportTable).getByText('maven-mirror')).toBeInTheDocument();
    expect(within(reportTable).getByText('7')).toBeInTheDocument();
    // 整仓跳过项以徽章呈现
    expect(screen.getByText('npm-hosted')).toBeInTheDocument();
    // 终态后清理存档
    expect(localStorage.getItem('jian.migrate.online.jobId')).toBeNull();
  });

  it('在线拉取：启动后首次进度快照返回前展示占位进度', async () => {
    vi.spyOn(api, 'previewNexusRepositories').mockResolvedValue(在线仓库含maven);
    const onlineSpy = vi
      .spyOn(api, 'migrateNexusOnline')
      .mockResolvedValue({ job_id: 'job-pending' });
    vi.spyOn(api, 'getMigrationJob').mockReturnValue(new Promise(() => {}));
    const user = userEvent.setup();
    renderPage();

    await user.type(screen.getByPlaceholderText('https://nexus.example'), 'https://nexus.example');
    await user.click(screen.getByRole('button', { name: '预览仓库' }));
    await waitFor(() => expect(screen.getByText('maven-releases')).toBeInTheDocument());
    await user.click(screen.getByRole('button', { name: '下一步：勾选执行' }));
    await user.click(screen.getByRole('checkbox', { name: /maven-releases/ }));
    await user.click(screen.getByRole('button', { name: '执行在线拉取' }));

    await waitFor(() => expect(onlineSpy).toHaveBeenCalled());
    expect(screen.getByText('任务已启动，正在加载首次进度快照')).toBeInTheDocument();
    expect(screen.queryByText('尚无迁移报告。')).toBeNull();
  });

  it('在线拉取：已有活跃迁移时前置拦截，不再发起新迁移', async () => {
    vi.mocked(api.listTasks).mockResolvedValueOnce([]).mockResolvedValueOnce([活跃迁移任务]);
    vi.spyOn(api, 'previewNexusRepositories').mockResolvedValue(在线仓库含maven);
    const onlineSpy = vi
      .spyOn(api, 'migrateNexusOnline')
      .mockResolvedValue({ job_id: 'job-blocked' });
    const user = userEvent.setup();
    renderPage();

    await user.type(screen.getByPlaceholderText('https://nexus.example'), 'https://nexus.example');
    await user.click(screen.getByRole('button', { name: '预览仓库' }));
    await waitFor(() => expect(screen.getByText('maven-releases')).toBeInTheDocument());
    await user.click(screen.getByRole('button', { name: '下一步：勾选执行' }));
    await user.click(screen.getByRole('checkbox', { name: /maven-releases/ }));
    await user.click(screen.getByRole('button', { name: '执行在线拉取' }));

    await waitFor(() => expect(api.listTasks).toHaveBeenCalledTimes(2));
    expect(onlineSpy).not.toHaveBeenCalled();
    expect(screen.getByText('已有迁移任务在途，请在任务中心或本页续看')).toBeInTheDocument();
  });

  it('离线搬运：活跃离线预览任务不拦截真正搬运', async () => {
    const 活跃离线预览任务: TaskRecord = {
      id: 'preview-active',
      kind: 'migration',
      state: 'running',
      label: '离线预览',
      started_at: 1,
      updated_at: 2,
    };
    vi.mocked(api.listTasks).mockResolvedValueOnce([]).mockResolvedValueOnce([活跃离线预览任务]);
    vi.spyOn(api, 'previewNexusOffline').mockResolvedValue({ job_id: 'prev-active-ok' });
    vi.spyOn(api, 'getTask').mockResolvedValue({
      id: 'prev-active-ok',
      kind: 'migration',
      state: 'succeeded',
      label: '离线预览',
      started_at: 1,
      updated_at: 2,
      migration: {
        ...任务完成,
        job_id: 'prev-active-ok',
        phase: 'done',
        total_assets: 1,
        done_assets: 1,
        migrated: 0,
        skipped: 0,
        repos: [],
        skipped_repos: [],
        offline_preview: [{ repo_name: 'maven-proxy', blob_count: 1, blobs: [] }],
      },
    });
    const migrateSpy = vi
      .spyOn(api, 'migrateNexusProxy')
      .mockResolvedValue({ job_id: 'off-preview-ok' });
    vi.spyOn(api, 'getMigrationJob').mockResolvedValue({ ...任务完成, job_id: 'off-preview-ok' });
    const user = userEvent.setup();
    renderPage();

    await user.click(screen.getByRole('radio', { name: /离线/ }));
    expect(screen.queryByPlaceholderText('https://nexus.example')).not.toBeInTheDocument();
    expect(screen.queryByPlaceholderText('例如 NEXUS_SRC')).not.toBeInTheDocument();
    await user.type(screen.getByPlaceholderText('/data/nexus/blobs/default'), '/data/blobs');
    await user.click(screen.getByRole('button', { name: '预览仓库' }));
    await waitFor(() => expect(screen.getByText('maven-proxy')).toBeInTheDocument());
    await user.click(screen.getByRole('button', { name: '下一步：勾选执行' }));
    await user.click(screen.getByRole('checkbox', { name: /maven-proxy/ }));
    expect(screen.getByPlaceholderText('/data/nexus/blobs/default')).toHaveValue('/data/blobs');
    await user.click(screen.getByRole('button', { name: '执行 proxy 搬运' }));

    await waitFor(() => expect(migrateSpy).toHaveBeenCalled());
    expect(migrateSpy).toHaveBeenCalledWith({
      offline_path: '/data/blobs',
      repositories: ['maven-proxy'],
    });
    expect(screen.queryByText('已有迁移任务在途，请在任务中心或本页续看')).not.toBeInTheDocument();
  });

  it('在线拉取：未填目标改名时省略 target（与源同名）', async () => {
    vi.spyOn(api, 'previewNexusRepositories').mockResolvedValue(在线仓库含maven);
    const onlineSpy = vi.spyOn(api, 'migrateNexusOnline').mockResolvedValue({ job_id: 'job-2' });
    vi.spyOn(api, 'getMigrationJob').mockResolvedValue({ ...任务完成, job_id: 'job-2' });
    const user = userEvent.setup();
    renderPage();

    await user.type(screen.getByPlaceholderText('https://nexus.example'), 'https://nexus.example');
    await user.click(screen.getByRole('button', { name: '预览仓库' }));
    await waitFor(() => expect(screen.getByText('maven-releases')).toBeInTheDocument());

    await user.click(screen.getByRole('button', { name: '下一步：勾选执行' }));
    await user.click(screen.getByRole('checkbox', { name: /maven-releases/ }));
    // 不填目标名，直接发起
    await user.click(screen.getByRole('button', { name: '执行在线拉取' }));

    await waitFor(() => expect(onlineSpy).toHaveBeenCalled());
    expect(onlineSpy).toHaveBeenCalledWith({
      base_url: 'https://nexus.example',
      auth_ref: undefined,
      repositories: [{ source: 'maven-releases' }],
    });
  });

  it('客户端重连：URL job_id 优先于服务端活跃任务和本地存档', async () => {
    localStorage.setItem('jian.migrate.online.jobId', 'job-archived');
    vi.mocked(api.listTasks).mockResolvedValue([活跃迁移任务]);
    const taskSpy = vi.spyOn(api, 'getTask').mockResolvedValue({
      id: 'job-url',
      kind: 'migration',
      state: 'succeeded',
      label: '在线拉取迁移',
      started_at: 1,
      updated_at: 2,
      migration: { ...任务完成, job_id: 'job-url' },
    });
    const jobSpy = vi.spyOn(api, 'getMigrationJob').mockResolvedValue({
      ...任务进行中,
      job_id: 'job-active',
    });

    renderPage('/migration?job_id=job-url');

    await waitFor(() => expect(taskSpy).toHaveBeenCalledWith('job-url'));
    await waitFor(() => expect(screen.getByText('已完成')).toBeInTheDocument());
    expect(api.listTasks).not.toHaveBeenCalled();
    expect(jobSpy).not.toHaveBeenCalled();
  });

  it('客户端重连：页面加载时从 listTasks 恢复服务端活跃迁移，优先于本地存档', async () => {
    localStorage.setItem('jian.migrate.online.jobId', 'job-archived');
    vi.mocked(api.listTasks).mockResolvedValue([活跃迁移任务]);
    const jobSpy = vi
      .spyOn(api, 'getMigrationJob')
      .mockResolvedValue({ ...任务进行中, job_id: 'job-active' });

    renderPage();

    await waitFor(() => expect(jobSpy).toHaveBeenCalledWith('job-active'));
    expect(jobSpy).not.toHaveBeenCalledWith('job-archived');
    await waitFor(() => expect(screen.getByText('下载搬运中')).toBeInTheDocument());
    expect(localStorage.getItem('jian.migrate.online.jobId')).toBe('job-active');
  });

  it('客户端重连：页面加载时存档 job_id 仍进行中则恢复轮询续看', async () => {
    // 预置存档：模拟刷新页面前已发起的任务。
    localStorage.setItem('jian.migrate.online.jobId', 'job-9');
    const jobSpy = vi
      .spyOn(api, 'getMigrationJob')
      .mockResolvedValue({ ...任务进行中, job_id: 'job-9' });
    renderPage();

    // 加载即拉取存档任务并切到报告步骤、展示进行中队列
    await waitFor(() => expect(jobSpy).toHaveBeenCalledWith('job-9'));
    await waitFor(() => expect(screen.getByText('下载搬运中')).toBeInTheDocument());
    expect(screen.getByText(/进度 3 \/ 7/)).toBeInTheDocument();
  });

  it('客户端重连：存档 job_id 已失效（404）则清理存档', async () => {
    localStorage.setItem('jian.migrate.online.jobId', 'job-gone');
    vi.spyOn(api, 'getMigrationJob').mockRejectedValue(
      new ApiError(404, 'not_found', '任务不存在'),
    );
    renderPage();

    await waitFor(() => expect(localStorage.getItem('jian.migrate.online.jobId')).toBeNull());
  });

  it('任务控制：进行中展示暂停 / 取消，点暂停调用暂停端点', async () => {
    localStorage.setItem('jian.migrate.online.jobId', 'job-1');
    // 稳定返回进行中态：避免轮询期间快照漂移，确保按钮态确定。
    vi.spyOn(api, 'getMigrationJob').mockResolvedValue(任务进行中);
    const pauseSpy = vi.spyOn(api, 'pauseMigrationJob').mockResolvedValue(undefined);
    const user = userEvent.setup();
    renderPage();

    await waitFor(() => expect(screen.getByText('下载搬运中')).toBeInTheDocument());
    // 进行中：展示「暂停」而非「继续」，暂停 / 取消均可用
    const pauseBtn = screen.getByRole('button', { name: '暂停' });
    expect(pauseBtn).toBeEnabled();
    expect(screen.getByRole('button', { name: '取消' })).toBeEnabled();
    expect(screen.queryByRole('button', { name: '继续' })).toBeNull();
    await user.click(pauseBtn);

    await waitFor(() => expect(pauseSpy).toHaveBeenCalledWith('job-1'));
  });

  it('任务控制：已暂停展示继续，点继续调用继续端点', async () => {
    localStorage.setItem('jian.migrate.online.jobId', 'job-1');
    vi.spyOn(api, 'getMigrationJob').mockResolvedValue(任务已暂停);
    const resumeSpy = vi.spyOn(api, 'resumeMigrationJob').mockResolvedValue(undefined);
    const user = userEvent.setup();
    renderPage();

    await waitFor(() => expect(screen.getByText('已暂停')).toBeInTheDocument());
    // 暂停态：展示「继续」而非「暂停」
    const resumeBtn = screen.getByRole('button', { name: '继续' });
    expect(resumeBtn).toBeEnabled();
    expect(screen.queryByRole('button', { name: '暂停' })).toBeNull();
    await user.click(resumeBtn);

    await waitFor(() => expect(resumeSpy).toHaveBeenCalledWith('job-1'));
  });

  it('任务控制：点取消调用取消端点', async () => {
    localStorage.setItem('jian.migrate.online.jobId', 'job-1');
    vi.spyOn(api, 'getMigrationJob').mockResolvedValue(任务进行中);
    const cancelSpy = vi.spyOn(api, 'cancelMigrationJob').mockResolvedValue(undefined);
    const user = userEvent.setup();
    renderPage();

    await waitFor(() => expect(screen.getByText('下载搬运中')).toBeInTheDocument());
    await user.click(screen.getByRole('button', { name: '取消' }));

    await waitFor(() => expect(cancelSpy).toHaveBeenCalledWith('job-1'));
  });

  it('任务控制：终态（已取消）下控制按钮全禁用', async () => {
    localStorage.setItem('jian.migrate.online.jobId', 'job-1');
    vi.spyOn(api, 'getMigrationJob').mockResolvedValue(任务已取消);
    renderPage();

    await waitFor(() => expect(screen.getByText('已取消')).toBeInTheDocument());
    // 终态：取消禁用；展示的是「暂停」（paused=false）且禁用
    expect(screen.getByRole('button', { name: '取消' })).toBeDisabled();
    expect(screen.getByRole('button', { name: '暂停' })).toBeDisabled();
  });

  it('预览失败展示错误文案', async () => {
    vi.spyOn(api, 'previewNexusRepositories').mockRejectedValue(
      new ApiError(502, 'bad_gateway', '连接源 Nexus 失败'),
    );
    const user = userEvent.setup();
    renderPage();

    await user.type(screen.getByPlaceholderText('https://nexus.example'), 'https://nexus.example');
    await user.click(screen.getByRole('button', { name: '预览仓库' }));

    await waitFor(() => expect(screen.getByText('连接源 Nexus 失败')).toBeInTheDocument());
  });
});
