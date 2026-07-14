// 用户管理测试：渲染默认场景用户列表、行内重置口令（M1-FR-045）。
import { fireEvent, screen, waitFor, within } from '@testing-library/react';
import { Route, Routes } from 'react-router-dom';
import { beforeEach, describe, expect, it } from 'vitest';
import { RequireAdmin } from '../routes/guards';
import { loginAs, renderWithProviders, resetScenario } from '../test/utils';
import { UsersPage } from './UsersPage';

function renderUsersPage() {
  renderWithProviders(
    <Routes>
      <Route path="/login" element={<div>登录页面</div>} />
      <Route
        path="/users"
        element={
          <RequireAdmin>
            <UsersPage />
          </RequireAdmin>
        }
      />
    </Routes>,
    { route: '/users' },
  );
}

describe('用户管理', () => {
  beforeEach(async () => {
    localStorage.clear();
    await resetScenario('default');
    await loginAs('admin', 'admin123');
  });

  it('渲染用户列表', async () => {
    renderUsersPage();
    expect(await screen.findByText('admin')).toBeInTheDocument();
    expect(screen.getByText('dev')).toBeInTheDocument();
  });

  it('创建用户输入使用独立 autocomplete 分区', async () => {
    renderUsersPage();
    fireEvent.click(await screen.findByRole('button', { name: '新增用户' }));
    expect(await screen.findByLabelText('用户名')).toHaveAttribute(
      'autocomplete',
      'section-create-user username',
    );
    expect(screen.getByLabelText('初始口令')).toHaveAttribute(
      'autocomplete',
      'section-create-user new-password',
    );
  });

  it('管理员重置口令使用独立 autocomplete 分区', async () => {
    renderUsersPage();
    const devCell = await screen.findByText('dev');
    const row = devCell.closest('tr');
    if (!row) throw new Error('未找到 dev 用户所在行');
    fireEvent.click(within(row).getByRole('button', { name: '重置口令' }));
    expect(await screen.findByLabelText('新口令')).toHaveAttribute(
      'autocomplete',
      'section-reset-password new-password',
    );
  });

  it('重置其他用户口令后保留当前会话并展示成功提示', async () => {
    renderUsersPage();
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
    expect(screen.queryByText('登录页面')).not.toBeInTheDocument();
    expect(localStorage.getItem('jianartifact.auth')).not.toBeNull();
  });

  it('重置自己的口令后清理会话并跳转登录页', async () => {
    renderUsersPage();
    const adminCell = await screen.findByText('admin');
    const row = adminCell.closest('tr');
    if (!row) throw new Error('未找到 admin 用户所在行');
    fireEvent.click(within(row).getByRole('button', { name: '重置口令' }));
    fireEvent.change(await screen.findByLabelText('新口令'), {
      target: { value: 'reset-admin-pass-1' },
    });
    fireEvent.click(screen.getByRole('button', { name: '重置' }));

    expect(await screen.findByText('登录页面')).toBeInTheDocument();
    expect(localStorage.getItem('jianartifact.auth')).toBeNull();
    expect(screen.queryByText('已重置 admin 的口令')).not.toBeInTheDocument();
  });

  it('按名称筛选后仅保留匹配的用户行', async () => {
    renderUsersPage();
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
