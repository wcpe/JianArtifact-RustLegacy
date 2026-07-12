// 仪表盘测试：管理员登录后展示各项概览数字。
import { screen } from '@testing-library/react';
import { beforeEach, describe, expect, it } from 'vitest';
import { loginAs, renderWithProviders, resetScenario } from '../test/utils';
import { DashboardPage } from './DashboardPage';

describe('仪表盘', () => {
  beforeEach(async () => {
    localStorage.clear();
    await resetScenario('default');
    await loginAs('admin', 'admin123');
  });

  it('展示仓库总数概览', async () => {
    renderWithProviders(<DashboardPage />);
    expect(await screen.findByText('仓库总数')).toBeInTheDocument();
    // 默认场景有六个仓库，数值唯一可断言
    expect(screen.getByText('6')).toBeInTheDocument();
    expect(screen.getByText('迁移任务数')).toBeInTheDocument();
  });
});
