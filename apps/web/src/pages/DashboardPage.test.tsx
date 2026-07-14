// 仪表盘测试：管理员登录后展示概览，并验证各卡片加载与失败互不遮蔽。
import { screen } from '@testing-library/react';
import { delay, http, HttpResponse } from 'msw';
import { beforeEach, describe, expect, it } from 'vitest';
import { server } from '../test/setup';
import { loginAs, renderWithProviders, resetScenario } from '../test/utils';
import { DashboardPage } from './DashboardPage';

describe('仪表盘', () => {
  beforeEach(async () => {
    localStorage.clear();
    await resetScenario('default');
    await loginAs('admin', 'admin123');
  });

  it('展示仓库总数概览', async () => {
    renderWithProviders(<DashboardPage />);
    expect(await screen.findByText('仓库总数')).toBeInTheDocument();
    // 默认场景有六个仓库，数值唯一可断言
    expect(screen.getByText('6')).toBeInTheDocument();
    expect(await screen.findByText('迁移任务数')).toBeInTheDocument();
  });

  it('仓库仍在加载时先展示已完成的 Token 卡', async () => {
    server.use(
      http.get('*/api/v1/repos', async () => {
        await delay(200);
        return HttpResponse.json({ items: [], page: 1, page_size: 1, total: 17 });
      }),
      http.get('*/api/v1/tokens', () =>
        HttpResponse.json({ items: [], page: 1, page_size: 1, total: 23 }),
      ),
      http.get('*/api/v1/migration/tasks', () =>
        HttpResponse.json({ items: [], page: 1, page_size: 1, total: 31 }),
      ),
    );

    renderWithProviders(<DashboardPage />);

    expect(await screen.findByText('23')).toBeInTheDocument();
    expect(screen.queryByText('17')).not.toBeInTheDocument();
    expect(await screen.findByText('17')).toBeInTheDocument();
  });

  it('迁移接口返回 501 时不遮蔽仓库与 Token 成功结果', async () => {
    server.use(
      http.get('*/api/v1/repos', () =>
        HttpResponse.json({ items: [], page: 1, page_size: 1, total: 41 }),
      ),
      http.get('*/api/v1/tokens', () =>
        HttpResponse.json({ items: [], page: 1, page_size: 1, total: 43 }),
      ),
      http.get('*/api/v1/migration/tasks', () =>
        HttpResponse.json(
          { code: 'NOT_IMPLEMENTED', message: '迁移任务接口暂未实现' },
          { status: 501 },
        ),
      ),
    );

    renderWithProviders(<DashboardPage />);

    expect(await screen.findByText('41')).toBeInTheDocument();
    expect(screen.getByText('43')).toBeInTheDocument();
    expect(await screen.findByText('迁移任务接口暂未实现')).toBeInTheDocument();
  });
});
