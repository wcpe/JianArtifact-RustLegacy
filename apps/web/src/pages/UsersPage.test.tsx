// 用户管理测试：渲染默认场景用户列表。
import { screen } from '@testing-library/react';
import { beforeEach, describe, expect, it } from 'vitest';
import { loginAs, renderWithProviders, resetScenario } from '../test/utils';
import { UsersPage } from './UsersPage';

describe('用户管理', () => {
  beforeEach(async () => {
    localStorage.clear();
    await resetScenario('default');
    await loginAs('admin', 'admin123');
  });

  it('渲染用户列表', async () => {
    renderWithProviders(<UsersPage />);
    expect(await screen.findByText('admin')).toBeInTheDocument();
    expect(screen.getByText('dev')).toBeInTheDocument();
  });
});
