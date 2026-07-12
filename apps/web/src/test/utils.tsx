// 页面测试工具：提供带路由与认证上下文的渲染器、mock 登录与场景切换 helper。
// 说明：页面测试经真实 fetch 打到 MSW devmock 后端（见 setup.ts），本文件只做上下文装配。
import type { ReactElement } from 'react';
import { render } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import type { ScenarioId } from '@jianartifact/devmock';
import { AuthProvider } from '../auth/AuthContext';
import { authApi } from '../api/endpoints';
import { setSessionToken } from '../api/client';

// 与 AuthContext 内部一致的 localStorage 键（测试直接写入以复原登录态）
const AUTH_STORAGE_KEY = 'jianartifact.auth';

// 渲染选项：初始路由，缺省根路径
interface RenderOptions {
  route?: string;
}

// 在 AuthProvider + MemoryRouter 中渲染组件；ui 可为单个页面或一组 Routes
export function renderWithProviders(ui: ReactElement, options: RenderOptions = {}) {
  const { route = '/' } = options;
  return render(
    <AuthProvider>
      <MemoryRouter initialEntries={[route]}>{ui}</MemoryRouter>
    </AuthProvider>,
  );
}

// 经 mock 后端登录并把会话写入 localStorage，使随后渲染的 AuthProvider 恢复为已登录态。
// 须在 renderWithProviders 之前调用（AuthProvider 在初始化时读取 localStorage）。
export async function loginAs(username: string, password: string): Promise<void> {
  const session = await authApi.login({ username, password });
  setSessionToken(session.token);
  const role: 'admin' | 'user' = username === 'admin' ? 'admin' : 'user';
  localStorage.setItem(
    AUTH_STORAGE_KEY,
    JSON.stringify({ token: session.token, user: { username, role } }),
  );
}

// 切换 mock 后端场景并整体重建其数据，保证各测试用例之间相互隔离
export async function resetScenario(id: ScenarioId = 'default'): Promise<void> {
  await fetch('/__devmock/scenario', {
    method: 'PUT',
    body: JSON.stringify({ scenario: id }),
  });
}
