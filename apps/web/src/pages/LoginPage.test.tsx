// 登录页测试：成功登录后跳转、失败展示后端错误消息。
import { fireEvent, screen } from '@testing-library/react';
import { Route, Routes } from 'react-router-dom';
import { beforeEach, describe, expect, it } from 'vitest';
import { renderWithProviders, resetScenario } from '../test/utils';
import { LoginPage } from './LoginPage';

describe('登录页', () => {
  beforeEach(async () => {
    localStorage.clear();
    await resetScenario('default');
  });

  it('登录成功后跳转到目标页', async () => {
    renderWithProviders(
      <Routes>
        <Route path="/login" element={<LoginPage />} />
        <Route path="/dashboard" element={<div>仪表盘首页</div>} />
      </Routes>,
      { route: '/login' },
    );
    fireEvent.change(screen.getByLabelText('用户名'), { target: { value: 'admin' } });
    fireEvent.change(screen.getByLabelText('口令'), { target: { value: 'admin123' } });
    fireEvent.click(screen.getByRole('button', { name: '登录' }));
    // 跳转后应渲染出目标页内容，登录表单不再存在
    expect(await screen.findByText('仪表盘首页')).toBeInTheDocument();
  });

  it('登录失败展示后端错误消息', async () => {
    renderWithProviders(<LoginPage />, { route: '/login' });
    fireEvent.change(screen.getByLabelText('用户名'), { target: { value: 'admin' } });
    fireEvent.change(screen.getByLabelText('口令'), { target: { value: 'wrong-password' } });
    fireEvent.click(screen.getByRole('button', { name: '登录' }));
    expect(await screen.findByText('用户名或口令无效')).toBeInTheDocument();
  });
});
