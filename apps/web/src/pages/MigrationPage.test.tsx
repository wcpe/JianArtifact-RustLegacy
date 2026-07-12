// 迁移向导测试：首步渲染源配置表单。
import { screen } from '@testing-library/react';
import { beforeEach, describe, expect, it } from 'vitest';
import { loginAs, renderWithProviders, resetScenario } from '../test/utils';
import { MigrationPage } from './MigrationPage';

describe('迁移向导', () => {
  beforeEach(async () => {
    // 清空在途任务标识，保证从首步开始
    localStorage.clear();
    await resetScenario('default');
    await loginAs('admin', 'admin123');
  });

  it('首步渲染源配置', () => {
    renderWithProviders(<MigrationPage />);
    expect(screen.getByText('选择迁移来源')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '开始发现' })).toBeInTheDocument();
  });
});
