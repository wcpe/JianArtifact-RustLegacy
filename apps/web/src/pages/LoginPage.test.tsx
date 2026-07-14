// 登录页测试：成功登录后跳转、失败展示后端错误消息。
import { http, HttpResponse } from 'msw';
import { fireEvent, screen, waitFor } from '@testing-library/react';
import { Route, Routes } from 'react-router-dom';
import { beforeEach, describe, expect, it } from 'vitest';
import { renderWithProviders, resetScenario } from '../test/utils';
import { RequireAdmin, RequireAuth } from '../routes/guards';
import { server } from '../test/setup';
import { LoginPage } from './LoginPage';

describe('登录页', () => {
  beforeEach(async () => {
    localStorage.clear();
    await resetScenario('default');
  });

  it('用户名和口令输入声明登录 autocomplete', () => {
    renderWithProviders(<LoginPage />, { route: '/login' });
    expect(screen.getByLabelText('用户名')).toHaveAttribute('autocomplete', 'username');
    expect(screen.getByLabelText('口令')).toHaveAttribute('autocomplete', 'current-password');
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

  it('持久化令牌与缓存身份仍会重新请求 auth/me', async () => {
    localStorage.setItem(
      'jianartifact.auth',
      JSON.stringify({
        token: 'persisted-old-token',
        user: { id: 999, username: 'cached-user', role: 'user' },
        identity_source: 'me',
      }),
    );
    let meRequests = 0;
    server.use(
      http.get('*/api/v1/auth/me', () => {
        meRequests += 1;
        return HttpResponse.json({
          id: 1,
          username: 'admin',
          role: 'admin',
          enabled: true,
          created_at: '2025-01-01T00:00:00Z',
        });
      }),
    );

    renderWithProviders(
      <Routes>
        <Route path="/login" element={<LoginPage />} />
        <Route
          path="/dashboard"
          element={
            <RequireAdmin>
              <div>管理员首页</div>
            </RequireAdmin>
          }
        />
      </Routes>,
      { route: '/dashboard' },
    );

    expect(await screen.findByText('管理员首页')).toBeInTheDocument();
    expect(meRequests).toBe(1);
  });

  it('恢复身份时 auth/me 返回 401 会清理状态并跳转登录页', async () => {
    localStorage.setItem(
      'jianartifact.auth',
      JSON.stringify({
        token: 'persisted-old-token',
        user: { id: 1, username: 'admin', role: 'admin' },
        identity_source: 'me',
      }),
    );
    server.use(
      http.get('*/api/v1/auth/me', () =>
        HttpResponse.json(
          { code: 'AUTH_UNAUTHENTICATED', message: '身份凭据无效或已失效' },
          { status: 401 },
        ),
      ),
    );

    renderWithProviders(
      <Routes>
        <Route path="/login" element={<LoginPage />} />
        <Route
          path="/dashboard"
          element={
            <RequireAuth>
              <div>仪表盘首页</div>
            </RequireAuth>
          }
        />
      </Routes>,
      { route: '/dashboard' },
    );

    expect(await screen.findByRole('button', { name: '登录' })).toBeInTheDocument();
    await waitFor(() => expect(localStorage.getItem('jianartifact.auth')).toBeNull());
    expect(screen.queryByText('仪表盘首页')).not.toBeInTheDocument();
  });

  it('登录后 auth/me 失败时清理令牌且不持久化会话', async () => {
    server.use(
      http.get('*/api/v1/auth/me', () =>
        HttpResponse.json(
          { code: 'AUTH_UNAUTHENTICATED', message: '身份凭据无效或已失效' },
          { status: 401 },
        ),
      ),
    );
    renderWithProviders(<LoginPage />, { route: '/login' });
    fireEvent.change(screen.getByLabelText('用户名'), { target: { value: 'admin' } });
    fireEvent.change(screen.getByLabelText('口令'), { target: { value: 'admin123' } });
    fireEvent.click(screen.getByRole('button', { name: '登录' }));
    expect(await screen.findByText('身份凭据无效或已失效')).toBeInTheDocument();
    expect(localStorage.getItem('jianartifact.auth')).toBeNull();
  });
});
