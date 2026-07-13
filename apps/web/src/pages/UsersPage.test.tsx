// 用户管理测试：渲染默认场景用户列表、行内重置口令（M1-FR-045）。
import { fireEvent, screen, waitFor, within } from '@testing-library/react';
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

  it('重置口令成功后关闭模态框并在页首提示', async () => {
    renderWithProviders(<UsersPage />);
    // 定位 dev 行并打开其重置口令模态框
    const devCell = await screen.findByText('dev');
    const row = devCell.closest('tr');
    if (!row) throw new Error('未找到 dev 用户所在行');
    fireEvent.click(within(row).getByRole('button', { name: '重置口令' }));
    fireEvent.change(await screen.findByLabelText('新口令'), {
      target: { value: 'reset-pass-1' },
    });
    fireEvent.click(screen.getByRole('button', { name: '重置' }));
    // 成功后模态框关闭并在页首提示
    expect(await screen.findByText('已重置 dev 的口令')).toBeInTheDocument();
    expect(screen.queryByLabelText('新口令')).not.toBeInTheDocument();
  });

  it('按名称筛选后仅保留匹配的用户行', async () => {
    renderWithProviders(<UsersPage />);
    // 先等全量用户列表渲染
    expect(await screen.findByText('dev')).toBeInTheDocument();
    expect(screen.getByText('admin')).toBeInTheDocument();
    // 输入 dev，防抖后经后端 q 筛选，仅保留 dev 行
    fireEvent.change(screen.getByPlaceholderText('按名称筛选…'), {
      target: { value: 'dev' },
    });
    // 等筛选结果落定：admin 消失且 dev 仍在（排除加载骨架的瞬时空窗）
    await waitFor(() => {
      expect(screen.queryByText('admin')).not.toBeInTheDocument();
      expect(screen.getByText('dev')).toBeInTheDocument();
    });
  });
});
