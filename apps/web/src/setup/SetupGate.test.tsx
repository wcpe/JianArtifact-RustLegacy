import { StrictMode } from 'react';
import { http, HttpResponse } from 'msw';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { beforeEach, describe, expect, it } from 'vitest';
import { server } from '../test/setup';
import { SetupGate } from './SetupGate';

function renderGate(route = '/dashboard', setupToken: string | null = null, strict = false) {
  const content = (
    <MemoryRouter initialEntries={[route]}>
      <SetupGate setupToken={setupToken}>
        <Routes>
          <Route path="/login" element={<div>登录路由</div>} />
          <Route path="*" element={<div>现有应用</div>} />
        </Routes>
      </SetupGate>
    </MemoryRouter>
  );
  return render(strict ? <StrictMode>{content}</StrictMode> : content);
}

describe('首启状态门', () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it('状态加载期间保留稳定高度且不放行应用', async () => {
    server.use(
      http.get('*/api/v1/setup/status', async () => {
        await new Promise(() => undefined);
        return HttpResponse.json({ setup_required: false });
      }),
    );

    renderGate();

    expect(screen.getByRole('status', { name: '正在检查初始化状态' })).toHaveClass('min-h-64');
    expect(screen.queryByText('现有应用')).not.toBeInTheDocument();
  });

  it('状态请求失败展示中文错误，重试成功后才放行', async () => {
    let attempts = 0;
    server.use(
      http.get('*/api/v1/setup/status', () => {
        attempts += 1;
        return attempts === 1
          ? HttpResponse.json({ code: 'TEMPORARY', message: '暂时不可用' }, { status: 503 })
          : HttpResponse.json({ setup_required: false });
      }),
    );

    renderGate();

    expect(await screen.findByText('无法检查首启初始化状态，请稍后重试。')).toBeInTheDocument();
    expect(screen.queryByText('现有应用')).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: '重试' }));
    expect(await screen.findByText('现有应用')).toBeInTheDocument();
  });

  it('严格模式下旧状态响应不会覆盖较新的放行结果', async () => {
    const resolvers: Array<(response: Response) => void> = [];
    let completed = 0;
    server.use(
      http.get('*/api/v1/setup/status', async () => {
        const response = await new Promise<Response>((resolve) => resolvers.push(resolve));
        completed += 1;
        return response;
      }),
    );

    renderGate('/dashboard', null, true);
    await waitFor(() => expect(resolvers).toHaveLength(2));
    resolvers[1]?.(HttpResponse.json({ setup_required: false }));
    expect(await screen.findByText('现有应用')).toBeInTheDocument();

    resolvers[0]?.(HttpResponse.json({ setup_required: true }));
    await waitFor(() => expect(completed).toBe(2));
    expect(screen.getByText('现有应用')).toBeInTheDocument();
    expect(screen.queryByRole('heading', { name: '初始化管理员' })).not.toBeInTheDocument();
  });

  it('无需初始化时放行现有应用，并把 setup 地址替换到登录页', async () => {
    server.use(
      http.get('*/api/v1/setup/status', () => HttpResponse.json({ setup_required: false })),
    );

    const view = renderGate();
    expect(await screen.findByText('现有应用')).toBeInTheDocument();
    view.unmount();

    renderGate('/setup');
    expect(await screen.findByText('登录路由')).toBeInTheDocument();
  });

  it('需要初始化时把任意路由替换到 setup，无令牌仅显示部署提示', async () => {
    server.use(
      http.get('*/api/v1/setup/status', () => HttpResponse.json({ setup_required: true })),
    );

    renderGate('/repos');

    expect(await screen.findByRole('heading', { name: '初始化管理员' })).toBeInTheDocument();
    expect(screen.getByText('使用部署提供的一次性链接')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: '创建管理员' })).not.toBeInTheDocument();
  });
});
