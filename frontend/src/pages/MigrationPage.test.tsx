// Nexus 迁移向导组件测试（FR-142）：
// 统一向导——源配置 → 索引双通道加载 / 在线预览 → 勾选（推断补填）→ 执行计划确认 → 执行进度 / 报告；
// 执行中刷新恢复；迁移历史（FR-140）可回看报告 + 一键重跑；凭据引用不回显明文。

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
  MigrationTaskDto,
  MigrationTaskDetailDto,
  NexusRepoSummary,
  OnlinePullJob,
  RepoIndexEntry,
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

const 在线仓库含maven: NexusRepoSummary[] = [
  { name: 'maven-releases', format: 'maven2', type: 'hosted', upstream_url: null },
  { name: 'npm-hosted', format: 'npm', type: 'hosted', upstream_url: null },
];

/** 离线索引双通道样本：一权威 proxy、一推断 raw（类型 / 上游为 null，待补填）。 */
const 索引双通道: RepoIndexEntry[] = [
  {
    name: 'maven-central',
    format: 'maven',
    format_source: 'authoritative',
    type: 'proxy',
    blob_count: 3,
    total_size: 6144,
    upstream_url: 'https://repo1.maven.org/maven2/',
  },
  {
    name: 'raw-hosted',
    format: 'raw',
    format_source: 'inferred',
    type: null,
    blob_count: 2,
    total_size: 4096,
    upstream_url: null,
  },
];

/** 任务进行中快照（downloading）。 */
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

/** 任务终态快照（done）。 */
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

/** 离线索引任务终态（携带 repo_index，不搬运）。 */
function 索引任务(jobId: string): OnlinePullJob {
  return {
    ...任务完成,
    job_id: jobId,
    total_assets: 0,
    done_assets: 0,
    migrated: 0,
    skipped: 0,
    repos: [],
    skipped_repos: [],
    repo_index: 索引双通道,
  };
}

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

const 历史列表: MigrationTaskDto[] = [
  {
    id: 'hist-interrupted',
    kind: 'offline_hosted',
    state: 'interrupted',
    label: '离线 hosted 搬运',
    phase: 'downloading',
    total_assets: 10,
    done_assets: 4,
    migrated: 4,
    skipped_existing: 0,
    skipped: 0,
    total_bytes: 10240,
    done_bytes: 4096,
    migrated_bytes: 4096,
    skipped_bytes: 0,
    error: null,
    started_at: '2026-07-01T00:00:00Z',
    updated_at: '2026-07-01T00:01:00Z',
    finished_at: null,
  },
  {
    id: 'hist-done',
    kind: 'online',
    state: 'succeeded',
    label: '在线拉取导入队列',
    phase: 'done',
    total_assets: 5,
    done_assets: 5,
    migrated: 5,
    skipped_existing: 0,
    skipped: 0,
    total_bytes: 5120,
    done_bytes: 5120,
    migrated_bytes: 5120,
    skipped_bytes: 0,
    error: null,
    started_at: '2026-06-30T00:00:00Z',
    updated_at: '2026-06-30T00:01:00Z',
    finished_at: '2026-06-30T00:01:00Z',
  },
];

const 历史详情: MigrationTaskDetailDto = {
  ...历史列表[1],
  repos: [
    {
      source_repo: 'maven-releases',
      target_repo: 'maven-mirror',
      format: 'maven',
      created: true,
      migrated_artifacts: 5,
      skipped_existing_artifacts: 0,
      skipped_artifacts: 0,
    },
  ],
};

describe('MigrationPage', () => {
  beforeEach(() => {
    vi.spyOn(api, 'listTasks').mockResolvedValue([]);
    vi.spyOn(api, 'getTask').mockRejectedValue(new ApiError(404, 'not_found', '任务不存在'));
    // 默认历史为空，避免每个用例都要 mock。
    vi.spyOn(api, 'listMigrationTasks').mockResolvedValue([]);
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.useRealTimers();
    localStorage.clear();
  });

  it('在线预览：填源地址后枚举仓库并展示列表（权威来源徽章）', async () => {
    const spy = vi.spyOn(api, 'previewNexusRepositories').mockResolvedValue(在线仓库含maven);
    const user = userEvent.setup();
    renderPage();

    await user.type(screen.getByPlaceholderText('https://nexus.example'), 'https://nexus.example');
    await user.click(screen.getByRole('button', { name: '预览仓库' }));

    await waitFor(() => expect(screen.getByText('maven-releases')).toBeInTheDocument());
    expect(screen.getByText('npm-hosted')).toBeInTheDocument();
    expect(spy).toHaveBeenCalledWith({ base_url: 'https://nexus.example', auth_ref: undefined });
    // 在线一律权威来源
    expect(screen.getAllByText('权威').length).toBeGreaterThan(0);
  });

  it('凭据引用输入框为口令型（不回显明文）', async () => {
    renderPage();
    const input = screen.getByPlaceholderText('例如 NEXUS_SRC');
    expect(input).toHaveAttribute('type', 'password');
  });

  it('离线索引双通道：加载索引后展示权威 / 推断来源与类型（FR-141）', async () => {
    const indexSpy = vi.spyOn(api, 'indexNexusOffline').mockResolvedValue({ job_id: 'idx-1' });
    vi.spyOn(api, 'getTask').mockImplementation(async (id: string) => {
      if (id !== 'idx-1') throw new ApiError(404, 'not_found', '任务不存在');
      return 任务详情(索引任务('idx-1'), '离线仓库索引');
    });
    const user = userEvent.setup();
    renderPage();

    await user.click(screen.getByRole('radio', { name: /离线/ }));
    expect(screen.queryByPlaceholderText('https://nexus.example')).not.toBeInTheDocument();
    await user.type(screen.getByPlaceholderText('/data/nexus/blobs/default'), '/data/blobs');
    // 离线可选源地址走权威通道
    await user.type(
      screen.getByPlaceholderText('https://nexus.example（留空则纯离线推断）'),
      'https://nexus.example',
    );
    await user.click(screen.getByRole('button', { name: '加载仓库索引' }));

    await waitFor(() => expect(screen.getByText('maven-central')).toBeInTheDocument());
    expect(screen.getByText('raw-hosted')).toBeInTheDocument();
    expect(indexSpy).toHaveBeenCalledWith({
      path: '/data/blobs',
      base_url: 'https://nexus.example',
      auth_ref: undefined,
    });
    // 权威 + 推断徽章并存
    expect(screen.getAllByText('权威').length).toBeGreaterThan(0);
    expect(screen.getAllByText('推断').length).toBeGreaterThan(0);
  });

  it('推断项未补填类型时不可进入执行计划，补填后可继续', async () => {
    vi.spyOn(api, 'indexNexusOffline').mockResolvedValue({ job_id: 'idx-2' });
    vi.spyOn(api, 'getTask').mockImplementation(async (id: string) => {
      if (id !== 'idx-2') throw new ApiError(404, 'not_found', '任务不存在');
      return 任务详情(索引任务('idx-2'), '离线仓库索引');
    });
    const user = userEvent.setup();
    renderPage();

    await user.click(screen.getByRole('radio', { name: /离线/ }));
    await user.type(screen.getByPlaceholderText('/data/nexus/blobs/default'), '/data/blobs');
    await user.click(screen.getByRole('button', { name: '加载仓库索引' }));
    await waitFor(() => expect(screen.getByText('raw-hosted')).toBeInTheDocument());
    await user.click(screen.getByRole('button', { name: '下一步：勾选仓库' }));

    // 勾选推断项 raw-hosted（尚未补填类型）
    await user.click(screen.getByRole('checkbox', { name: /raw-hosted/ }));
    const nextPlan = screen.getByRole('button', { name: '下一步：确认计划' });
    expect(nextPlan).toBeDisabled();
    expect(
      screen.getByText('有勾选的推断仓库尚未确认类型，请先补填后再继续。'),
    ).toBeInTheDocument();

    // 为推断项补填类型 hosted → 可继续
    await user.click(screen.getByRole('textbox', { name: 'raw-hosted 仓库类型' }));
    await user.click(await screen.findByRole('option', { name: 'hosted' }));
    await waitFor(() => expect(nextPlan).toBeEnabled());
  });

  it('离线全流程：索引 → 勾选权威 proxy → 计划确认 → 发起 proxy 搬运并进入进度', async () => {
    vi.spyOn(api, 'indexNexusOffline').mockResolvedValue({ job_id: 'idx-3' });
    let 搬运读取 = 0;
    vi.spyOn(api, 'getTask').mockImplementation(async (id: string) => {
      if (id === 'idx-3') return 任务详情(索引任务('idx-3'), '离线仓库索引');
      if (id === 'off-1') {
        搬运读取 += 1;
        return 任务详情(
          搬运读取 === 1
            ? { ...任务进行中, job_id: 'off-1', current_repo: 'maven-central' }
            : { ...任务完成, job_id: 'off-1' },
          '离线 proxy 搬运',
        );
      }
      throw new ApiError(404, 'not_found', '任务不存在');
    });
    const proxySpy = vi.spyOn(api, 'migrateNexusProxy').mockResolvedValue({ job_id: 'off-1' });
    const user = userEvent.setup();
    renderPage();

    await user.click(screen.getByRole('radio', { name: /离线/ }));
    await user.type(screen.getByPlaceholderText('/data/nexus/blobs/default'), '/data/blobs');
    await user.click(screen.getByRole('button', { name: '加载仓库索引' }));
    await waitFor(() => expect(screen.getByText('maven-central')).toBeInTheDocument());
    await user.click(screen.getByRole('button', { name: '下一步：勾选仓库' }));

    // 勾选权威 proxy 项（类型只读、无需补填）
    await user.click(screen.getByRole('checkbox', { name: /maven-central/ }));
    await user.click(screen.getByRole('button', { name: '下一步：确认计划' }));

    // 执行计划展示方式为 proxy 搬运
    await waitFor(() => expect(screen.getByText('执行计划')).toBeInTheDocument());
    expect(screen.getByText('proxy 搬运')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: '开始迁移' }));

    await waitFor(() =>
      expect(proxySpy).toHaveBeenCalledWith({
        offline_path: '/data/blobs',
        repositories: ['maven-central'],
      }),
    );
    await waitFor(() => expect(screen.getByText('离线 proxy 搬运')).toBeInTheDocument());
    await waitFor(() => expect(screen.getByText('已完成')).toBeInTheDocument(), { timeout: 3000 });
    expect(screen.getByText('总数 7')).toBeInTheDocument();
  });

  it('在线全流程：预览 → 勾选改名 → 计划确认 → 发起在线拉取，轮询 done 展示报告', async () => {
    vi.spyOn(api, 'previewNexusRepositories').mockResolvedValue(在线仓库含maven);
    const onlineSpy = vi.spyOn(api, 'migrateNexusOnline').mockResolvedValue({ job_id: 'job-1' });
    vi.spyOn(api, 'getMigrationJob').mockResolvedValueOnce(任务进行中).mockResolvedValue(任务完成);
    const user = userEvent.setup();
    renderPage();

    await user.type(screen.getByPlaceholderText('https://nexus.example'), 'https://nexus.example');
    await user.click(screen.getByRole('button', { name: '预览仓库' }));
    await waitFor(() => expect(screen.getByText('maven-releases')).toBeInTheDocument());
    await user.click(screen.getByRole('button', { name: '下一步：勾选仓库' }));
    await user.click(screen.getByRole('checkbox', { name: /maven-releases/ }));
    await user.type(
      screen.getByRole('textbox', { name: 'maven-releases 目标仓库名' }),
      'maven-mirror',
    );
    await user.click(screen.getByRole('button', { name: '下一步：确认计划' }));

    // 计划页展示改名后的目标
    await waitFor(() => expect(screen.getByText('执行计划')).toBeInTheDocument());
    expect(screen.getByText('在线拉取')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: '开始迁移' }));

    await waitFor(() => expect(onlineSpy).toHaveBeenCalled());
    expect(onlineSpy).toHaveBeenCalledWith({
      base_url: 'https://nexus.example',
      auth_ref: undefined,
      repositories: [{ source: 'maven-releases', target: 'maven-mirror' }],
    });
    expect(localStorage.getItem('jian.migrate.online.jobId')).toBe('job-1');
    await waitFor(() => expect(screen.getByText('下载搬运中')).toBeInTheDocument());
    await waitFor(() => expect(screen.getByText('已完成')).toBeInTheDocument(), { timeout: 3000 });
    const reportTable = screen.getByText('maven-releases', { selector: 'td' }).closest('table')!;
    expect(within(reportTable).getByText('maven-mirror')).toBeInTheDocument();
    expect(localStorage.getItem('jian.migrate.online.jobId')).toBeNull();
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
    await user.click(screen.getByRole('button', { name: '下一步：勾选仓库' }));
    await user.click(screen.getByRole('checkbox', { name: /maven-releases/ }));
    await user.click(screen.getByRole('button', { name: '下一步：确认计划' }));
    await user.click(screen.getByRole('button', { name: '开始迁移' }));

    await waitFor(() => expect(onlineSpy).toHaveBeenCalled());
    expect(onlineSpy).toHaveBeenCalledWith({
      base_url: 'https://nexus.example',
      auth_ref: undefined,
      repositories: [{ source: 'maven-releases' }],
    });
  });

  it('单飞拦截：已有活跃迁移时不发起新迁移', async () => {
    const 活跃迁移任务: TaskRecord = {
      id: 'job-active',
      kind: 'migration',
      state: 'running',
      label: '在线拉取迁移',
      started_at: 1,
      updated_at: 2,
    };
    vi.mocked(api.listTasks).mockResolvedValueOnce([]).mockResolvedValueOnce([活跃迁移任务]);
    vi.spyOn(api, 'previewNexusRepositories').mockResolvedValue(在线仓库含maven);
    const onlineSpy = vi.spyOn(api, 'migrateNexusOnline').mockResolvedValue({ job_id: 'blocked' });
    const user = userEvent.setup();
    renderPage();

    await user.type(screen.getByPlaceholderText('https://nexus.example'), 'https://nexus.example');
    await user.click(screen.getByRole('button', { name: '预览仓库' }));
    await waitFor(() => expect(screen.getByText('maven-releases')).toBeInTheDocument());
    await user.click(screen.getByRole('button', { name: '下一步：勾选仓库' }));
    await user.click(screen.getByRole('checkbox', { name: /maven-releases/ }));
    await user.click(screen.getByRole('button', { name: '下一步：确认计划' }));
    await user.click(screen.getByRole('button', { name: '开始迁移' }));

    await waitFor(() => expect(api.listTasks).toHaveBeenCalledTimes(2));
    expect(onlineSpy).not.toHaveBeenCalled();
    expect(screen.getByText('已有迁移任务在途，请在任务中心或本页续看')).toBeInTheDocument();
  });

  it('执行中恢复：URL job_id 进入时经任务详情恢复到进度视图', async () => {
    const taskSpy = vi
      .spyOn(api, 'getTask')
      .mockResolvedValue(任务详情({ ...任务进行中, job_id: 'off-live' }, '离线 proxy 搬运'));
    renderPage('/migration?job_id=off-live');

    await waitFor(() => expect(taskSpy).toHaveBeenCalledWith('off-live'));
    await waitFor(() => expect(screen.getByText('离线 proxy 搬运')).toBeInTheDocument());
    expect(screen.getByText('下载搬运中')).toBeInTheDocument();
  });

  it('执行中恢复：加载时从 listTasks 恢复服务端活跃迁移', async () => {
    const 活跃迁移任务: TaskRecord = {
      id: 'job-active',
      kind: 'migration',
      state: 'running',
      label: '在线拉取迁移',
      started_at: 1,
      updated_at: 2,
    };
    vi.mocked(api.listTasks).mockResolvedValue([活跃迁移任务]);
    const jobSpy = vi
      .spyOn(api, 'getMigrationJob')
      .mockResolvedValue({ ...任务进行中, job_id: 'job-active' });
    renderPage();

    await waitFor(() => expect(jobSpy).toHaveBeenCalledWith('job-active'));
    await waitFor(() => expect(screen.getByText('下载搬运中')).toBeInTheDocument());
    expect(localStorage.getItem('jian.migrate.online.jobId')).toBe('job-active');
  });

  it('任务控制：进行中点暂停调用暂停端点', async () => {
    localStorage.setItem('jian.migrate.online.jobId', 'job-1');
    vi.spyOn(api, 'getMigrationJob').mockResolvedValue(任务进行中);
    const pauseSpy = vi.spyOn(api, 'pauseMigrationJob').mockResolvedValue(undefined);
    const user = userEvent.setup();
    renderPage();

    await waitFor(() => expect(screen.getByText('下载搬运中')).toBeInTheDocument());
    await user.click(screen.getByRole('button', { name: '暂停' }));
    await waitFor(() => expect(pauseSpy).toHaveBeenCalledWith('job-1'));
  });

  it('迁移历史：列出含 interrupted 的历史，点开展示报告', async () => {
    vi.mocked(api.listMigrationTasks).mockResolvedValue(历史列表);
    const detailSpy = vi.spyOn(api, 'getMigrationTask').mockResolvedValue(历史详情);
    const user = userEvent.setup();
    renderPage();

    await waitFor(() => expect(screen.getByText('迁移历史')).toBeInTheDocument());
    expect(screen.getByText('已中断')).toBeInTheDocument();

    // 点开已完成任务的报告
    const doneRow = screen.getByText('在线拉取导入队列').closest('tr')!;
    await user.click(within(doneRow).getByRole('button', { name: '查看报告' }));

    await waitFor(() => expect(detailSpy).toHaveBeenCalledWith('hist-done'));
    await waitFor(() => expect(screen.getByText('分仓库报告')).toBeInTheDocument());
    expect(screen.getByText('maven-mirror')).toBeInTheDocument();
  });

  it('迁移历史：interrupted 任务可一键重跑并进入进度续看', async () => {
    vi.mocked(api.listMigrationTasks).mockResolvedValue(历史列表);
    const rerunSpy = vi.spyOn(api, 'rerunMigrationTask').mockResolvedValue({ job_id: 'rerun-1' });
    vi.spyOn(api, 'getMigrationJob').mockResolvedValue({ ...任务进行中, job_id: 'rerun-1' });
    const user = userEvent.setup();
    renderPage();

    await waitFor(() => expect(screen.getByText('已中断')).toBeInTheDocument());
    const interruptedRow = screen.getByText('离线 hosted 搬运').closest('tr')!;
    await user.click(within(interruptedRow).getByRole('button', { name: '重跑续传' }));

    await waitFor(() => expect(rerunSpy).toHaveBeenCalledWith('hist-interrupted'));
    await waitFor(() => expect(screen.getByText('下载搬运中')).toBeInTheDocument());
    expect(localStorage.getItem('jian.migrate.online.jobId')).toBe('rerun-1');
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
