// 进度步测试：running 提供暂停控制，暂停后状态徽标切换为已暂停并提供继续。
import { fireEvent, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it } from 'vitest';
import { migrationApi } from '../../api/endpoints';
import { loginAs, renderWithProviders, resetScenario } from '../../test/utils';
import { ProgressStep } from './ProgressStep';

// 走真实 devmock 流程创建一个 running 任务，返回任务标识
async function createRunningTask(): Promise<string> {
  const discovery = await migrationApi.discover({
    kind: 'native-dir',
    path: '/srv/nexus/data',
    url: 'https://nexus.example.com',
    username: 'nexus-admin',
    password: 'nexus-pass',
  });
  const plan = await migrationApi.plan({
    discovery_id: discovery.discovery_id,
    mappings: discovery.repositories.map((repo) => ({
      source: repo.name,
      target: repo.name,
      excluded: false,
    })),
  });
  const task = await migrationApi.createTask(plan.plan_id);
  return task.task_id;
}

describe('进度步', () => {
  beforeEach(async () => {
    localStorage.clear();
    await resetScenario('default');
    await loginAs('admin', 'admin123');
  });

  it('running 展示暂停按钮，点暂停后徽标变已暂停并提供继续', async () => {
    const taskId = await createRunningTask();
    renderWithProviders(
      <ProgressStep taskId={taskId} onViewReport={() => undefined} onRestart={() => undefined} />,
    );
    // 任务创建后即 running：暂停/取消可用
    const pauseButton = await screen.findByRole('button', { name: '暂停' });
    expect(screen.getByText('运行中')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '取消' })).toBeInTheDocument();
    fireEvent.click(pauseButton);
    // devmock 暂停立即生效：徽标切换、控制按钮切换为 继续/取消
    expect(await screen.findByText('已暂停')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '继续' })).toBeInTheDocument();
  });

  it('running 在进度条旁展示迁移中阶段说明', async () => {
    const taskId = await createRunningTask();
    renderWithProviders(
      <ProgressStep taskId={taskId} onViewReport={() => undefined} onRestart={() => undefined} />,
    );
    // 阶段说明比徽标更完整地陈述当前处境
    expect(await screen.findByText(/迁移进行中/)).toBeInTheDocument();
    // 未到终态：查看报告禁用
    expect(screen.getByRole('button', { name: '查看报告' })).toBeDisabled();
  });

  it('终态任务恢复：展示部分完成阶段、放开查看报告并提供重试', async () => {
    // 直接以终态任务标识挂载即刷新恢复场景：轮询取回终态并停止
    await resetScenario('migration-partial');
    renderWithProviders(
      <ProgressStep
        taskId="task_part_1"
        onViewReport={() => undefined}
        onRestart={() => undefined}
      />,
    );
    const viewReport = await screen.findByRole('button', { name: '查看报告' });
    expect(viewReport).toBeEnabled();
    expect(screen.getByText(/部分对象未成功迁移/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '重试' })).toBeInTheDocument();
  });
});
