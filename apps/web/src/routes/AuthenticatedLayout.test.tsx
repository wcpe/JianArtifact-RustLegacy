// 登录后布局测试：分组导航、用户菜单触发器与搜索框版式位。
import { screen } from '@testing-library/react';
import { Route, Routes } from 'react-router-dom';
import { beforeEach, describe, expect, it } from 'vitest';
import { loginAs, renderWithProviders, resetScenario } from '../test/utils';
import { AuthenticatedLayout } from './AuthenticatedLayout';

// 渲染登录后布局（子路由给占位内容）
function renderLayout() {
  renderWithProviders(
    <Routes>
      <Route element={<AuthenticatedLayout />}>
        <Route path="/" element={<div>占位内容</div>} />
      </Route>
    </Routes>,
  );
}

describe('登录后布局', () => {
  beforeEach(async () => {
    localStorage.clear();
    await resetScenario('default');
    await loginAs('admin', 'admin123');
  });

  it('渲染分组导航、用户菜单触发器与搜索框', () => {
    renderLayout();
    // 分组导航项
    expect(screen.getByRole('link', { name: '仪表盘' })).toBeInTheDocument();
    expect(screen.getByRole('link', { name: '仓库' })).toBeInTheDocument();
    // 顶栏用户菜单触发器（头像 + 用户名，含 admin）
    expect(screen.getByRole('button', { name: /admin/ })).toBeInTheDocument();
    // 搜索框版式位
    expect(screen.getByLabelText('搜索制品')).toBeInTheDocument();
  });
});
