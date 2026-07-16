import { http, HttpResponse } from 'msw';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { server } from '../test/setup';
import { SetupPage } from './SetupPage';

function fillForm(password: string, confirmation = password) {
  fireEvent.change(screen.getByLabelText('用户名'), { target: { value: 'first-admin' } });
  fireEvent.change(screen.getByLabelText('新口令'), { target: { value: password } });
  fireEvent.change(screen.getByLabelText('确认口令'), { target: { value: confirmation } });
}

function renderPage(onSetupComplete = vi.fn(async () => true)) {
  render(
    <MemoryRouter initialEntries={['/setup']}>
      <Routes>
        <Route
          path="/setup"
          element={
            <SetupPage setupToken="component-memory-token" onSetupComplete={onSetupComplete} />
          }
        />
        <Route path="/login" element={<div>登录页</div>} />
      </Routes>
    </MemoryRouter>,
  );
  return onSetupComplete;
}

describe('首启管理员表单', () => {
  beforeEach(() => {
    server.use(http.post('*/api/v1/setup', () => new HttpResponse(null, { status: 204 })));
  });

  it('输入声明独立 autocomplete section，且口令按 Unicode 字符数校验 11/12 边界', async () => {
    renderPage();

    expect(screen.getByLabelText('用户名')).toHaveAttribute(
      'autocomplete',
      'section-setup username',
    );
    expect(screen.getByLabelText('用户名')).toHaveFocus();
    expect(screen.getByLabelText('新口令')).toHaveAttribute(
      'autocomplete',
      'section-setup new-password',
    );
    expect(screen.getByLabelText('确认口令')).toHaveAttribute(
      'autocomplete',
      'section-setup new-password',
    );

    fillForm('密'.repeat(11));
    fireEvent.click(screen.getByRole('button', { name: '创建管理员' }));
    expect(await screen.findByText('新口令至少需要 12 个字符')).toBeInTheDocument();

    fillForm('密'.repeat(12));
    fireEvent.click(screen.getByRole('button', { name: '创建管理员' }));
    expect(await screen.findByText('登录页')).toBeInTheDocument();
  });

  it('两次口令不一致时拒绝提交', async () => {
    renderPage();
    fillForm('密'.repeat(12), '安'.repeat(12));

    fireEvent.click(screen.getByRole('button', { name: '创建管理员' }));

    expect(await screen.findByText('两次输入的口令不一致')).toBeInTheDocument();
  });

  it('提交期间禁用按钮，成功刷新状态并替换到登录页', async () => {
    let finishRequest: () => void = () => void 0;
    const requestGate = new Promise<void>((resolve) => {
      finishRequest = resolve;
    });
    server.use(
      http.post('*/api/v1/setup', async () => {
        await requestGate;
        return new HttpResponse(null, { status: 204 });
      }),
    );
    const onSetupComplete = renderPage();
    fillForm('密'.repeat(12));

    fireEvent.click(screen.getByRole('button', { name: '创建管理员' }));
    expect(screen.getByRole('button', { name: '创建中…' })).toBeDisabled();
    finishRequest();

    expect(await screen.findByText('登录页')).toBeInTheDocument();
    expect(onSetupComplete).toHaveBeenCalledTimes(1);
  });

  it('状态刷新失败时不误报完成或跳转登录', async () => {
    renderPage(vi.fn(async () => false));
    fillForm('密'.repeat(12));

    fireEvent.click(screen.getByRole('button', { name: '创建管理员' }));

    expect(
      await screen.findByText('管理员已创建，但初始化状态刷新失败，请确认状态后再登录。'),
    ).toBeInTheDocument();
    expect(screen.queryByText('登录页')).not.toBeInTheDocument();
  });

  it('后端错误以清晰中文展示且保留表单', async () => {
    server.use(
      http.post('*/api/v1/setup', () =>
        HttpResponse.json(
          { code: 'SETUP_UNAVAILABLE', message: '初始化入口不可用，请重新获取部署链接' },
          { status: 404 },
        ),
      ),
    );
    renderPage();
    fillForm('密'.repeat(12));

    fireEvent.click(screen.getByRole('button', { name: '创建管理员' }));

    expect(await screen.findByText('初始化入口不可用，请重新获取部署链接')).toBeInTheDocument();
    await waitFor(() => expect(screen.getByRole('button', { name: '创建管理员' })).toBeEnabled());
  });
});
