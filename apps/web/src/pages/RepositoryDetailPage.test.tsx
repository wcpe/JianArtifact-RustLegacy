// 仓库详情测试：文件标签默认展示根目录条目。
import { screen } from '@testing-library/react';
import { Route, Routes } from 'react-router-dom';
import { beforeEach, describe, expect, it } from 'vitest';
import { loginAs, renderWithProviders, resetScenario } from '../test/utils';
import { RepositoryDetailPage } from './RepositoryDetailPage';

describe('仓库详情', () => {
  beforeEach(async () => {
    localStorage.clear();
    await resetScenario('default');
    await loginAs('admin', 'admin123');
  });

  it('文件标签展示根目录条目', async () => {
    renderWithProviders(
      <Routes>
        <Route path="/repos/:name" element={<RepositoryDetailPage />} />
      </Routes>,
      { route: '/repos/raw-hosted' },
    );
    expect(await screen.findByRole('heading', { name: 'raw-hosted' })).toBeInTheDocument();
    // raw-hosted 根下有 docs 与 tools 两个目录
    expect(await screen.findByText('docs')).toBeInTheDocument();
    expect(screen.getByText('tools')).toBeInTheDocument();
  });
});
