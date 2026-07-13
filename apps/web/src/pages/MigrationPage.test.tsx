// 迁移向导测试：首步渲染、在线拉取账号口令输入、hash + 缓存恢复与损坏回退。
import { fireEvent, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it } from 'vitest';
import type { components } from '@jianartifact/devmock/schema';
import { loginAs, renderWithProviders, resetScenario } from '../test/utils';
import { MIGRATION_TASK_KEY } from './migration/types';
import { DISCOVERY_KEY, PLAN_KEY } from './migration/wizardCache';
import { MigrationPage } from './MigrationPage';

type DiscoveryResult = components['schemas']['DiscoveryResult'];
type MigrationPlan = components['schemas']['MigrationPlan'];

// 最小可用的发现结果夹具（含一个源仓库供映射明细 join）
function discoveryFixture(): DiscoveryResult {
  return {
    discovery_id: 'disc_test',
    snapshot_time: '2026-07-01T00:00:00Z',
    degraded_bare_blob: false,
    repositories: [
      {
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
    ],
    unsupported: [],
  };
}

// 最小可用的计划夹具（映射到改名后的目标仓库）
function planFixture(): MigrationPlan {
  return {
    plan_id: 'plan_test',
    mappings: [{ source: 'maven-legacy', target: 'maven-new', excluded: false }],
    conflicts: [],
    blocked: false,
    object_count: 1280,
    total_bytes: 2_147_483_648,
    estimated_disk_bytes: 2_576_980_378,
    disk_sufficient: true,
    unsupported: [],
  };
}

describe('迁移向导', () => {
  beforeEach(async () => {
    // 清空在途任务标识与中间态缓存、复位 URL 锚点，保证从首步开始
    localStorage.clear();
    sessionStorage.clear();
    window.history.replaceState(null, '', '/');
    await resetScenario('default');
    await loginAs('admin', 'admin123');
  });

  it('首步渲染源配置', () => {
    renderWithProviders(<MigrationPage />);
    expect(screen.getByText('选择迁移来源')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '开始发现' })).toBeInTheDocument();
  });

  it('native-dir 的 Nexus 地址选填，填写后出现账号口令输入与帮助文案', () => {
    renderWithProviders(<MigrationPage />);
    // 默认 kind=native-dir：地址选填，留空不出现账号口令输入
    const urlInput = screen.getByLabelText('Nexus 地址（选填）');
    expect(urlInput).not.toBeRequired();
    expect(screen.queryByLabelText('Nexus 口令')).not.toBeInTheDocument();
    // 填写地址后出现账号口令输入与「不保存」帮助文案
    fireEvent.change(urlInput, { target: { value: 'https://nexus.example.com' } });
    expect(screen.getByLabelText('Nexus 用户名')).toBeInTheDocument();
    expect(screen.getByLabelText('Nexus 口令')).toBeInTheDocument();
    expect(screen.getByText(/服务端不保存、不写入日志与迁移报告/)).toBeInTheDocument();
  });

  it('带 #plan 锚点且缓存齐时恢复到计划步并渲染映射明细', () => {
    sessionStorage.setItem(DISCOVERY_KEY, JSON.stringify(discoveryFixture()));
    sessionStorage.setItem(PLAN_KEY, JSON.stringify(planFixture()));
    window.history.replaceState(null, '', '/#plan');
    renderWithProviders(<MigrationPage />);
    expect(screen.getByRole('button', { name: '开始迁移' })).toBeInTheDocument();
    expect(screen.getByText('仓库映射明细（1）')).toBeInTheDocument();
    expect(screen.getByText('maven-legacy')).toBeInTheDocument();
    expect(screen.getByText('maven-new')).toBeInTheDocument();
  });

  it('缓存损坏时安全回退到源配置步', () => {
    // 发现缓存写入非法 JSON：#plan 缺依赖数据，应回退首步
    sessionStorage.setItem(DISCOVERY_KEY, '{损坏的缓存');
    sessionStorage.setItem(PLAN_KEY, JSON.stringify(planFixture()));
    window.history.replaceState(null, '', '/#plan');
    renderWithProviders(<MigrationPage />);
    expect(screen.getByText('选择迁移来源')).toBeInTheDocument();
  });

  it('全流程各步展示处理态并推进到进度', async () => {
    renderWithProviders(<MigrationPage />);
    // 步骤一 源配置 → 发现：点开始发现后展示解析处理态卡片
    fireEvent.change(screen.getByLabelText('数据目录路径'), {
      target: { value: '/srv/nexus/data' },
    });
    fireEvent.change(screen.getByLabelText('Nexus 地址（选填）'), {
      target: { value: 'https://nexus.example.com' },
    });
    fireEvent.click(screen.getByRole('button', { name: '开始发现' }));
    expect(screen.getByText(/正在解析数据目录并枚举可迁对象/)).toBeInTheDocument();
    // 发现完成进入发现步，处理态消失
    expect(await screen.findByRole('button', { name: '生成计划' })).toBeInTheDocument();
    expect(screen.queryByText(/正在解析数据目录并枚举可迁对象/)).not.toBeInTheDocument();

    // 步骤二 发现 → 计划：点生成计划后展示计算处理态卡片
    fireEvent.click(screen.getByRole('button', { name: '生成计划' }));
    expect(screen.getByText(/正在计算映射与冲突/)).toBeInTheDocument();
    expect(await screen.findByRole('button', { name: '开始迁移' })).toBeInTheDocument();
    expect(screen.queryByText(/正在计算映射与冲突/)).not.toBeInTheDocument();

    // 步骤三 计划 → 进度：点开始迁移后展示创建任务处理态卡片
    fireEvent.click(screen.getByRole('button', { name: '开始迁移' }));
    expect(screen.getByText(/正在创建迁移任务/)).toBeInTheDocument();
    expect(await screen.findByText('迁移进度')).toBeInTheDocument();
    expect(screen.queryByText(/正在创建迁移任务/)).not.toBeInTheDocument();
  });

  it('在途任务标识恢复到进度页并展示迁移中阶段', async () => {
    // migration-running 场景种子含运行中任务 task_run_1；写入在途任务标识模拟刷新恢复
    await resetScenario('migration-running');
    localStorage.setItem(MIGRATION_TASK_KEY, 'task_run_1');
    renderWithProviders(<MigrationPage />);
    // 恢复优先级：有在途任务 → 进度页（不回源配置）
    expect(await screen.findByText('迁移进度')).toBeInTheDocument();
    expect(screen.getByText(/迁移进行中/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '重新开始' })).toBeInTheDocument();
    expect(screen.queryByLabelText('数据目录路径')).not.toBeInTheDocument();
  });

  it('终态在途任务恢复到进度页并放开查看报告', async () => {
    // migration-partial 场景种子含部分完成终态任务 task_part_1
    await resetScenario('migration-partial');
    localStorage.setItem(MIGRATION_TASK_KEY, 'task_part_1');
    renderWithProviders(<MigrationPage />);
    // 终态：查看报告放开、阶段说明为部分完成
    const viewReport = await screen.findByRole('button', { name: '查看报告' });
    expect(viewReport).toBeEnabled();
    expect(screen.getByText(/部分对象未成功迁移/)).toBeInTheDocument();
  });

  it('进度页遇在途任务不存在时可重新开始回到源配置', async () => {
    // 在途任务标识指向已不存在的任务（服务重启 / 任务清理）：轮询 404 后应给出逃生出口
    localStorage.setItem(MIGRATION_TASK_KEY, 'task_missing');
    renderWithProviders(<MigrationPage />);
    expect(await screen.findByText('任务不存在')).toBeInTheDocument();
    // 点重新开始：清空在途任务标识与缓存，回到源配置步
    fireEvent.click(screen.getByRole('button', { name: '重新开始' }));
    expect(screen.getByText('选择迁移来源')).toBeInTheDocument();
    expect(localStorage.getItem(MIGRATION_TASK_KEY)).toBeNull();
  });
});
