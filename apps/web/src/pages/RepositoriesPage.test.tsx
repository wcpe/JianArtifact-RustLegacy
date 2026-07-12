// 仓库列表测试：渲染默认场景中的仓库行。
import { screen } from '@testing-library/react';
import { beforeEach, describe, expect, it } from 'vitest';
import { loginAs, renderWithProviders, resetScenario } from '../test/utils';
import { RepositoriesPage } from './RepositoriesPage';

describe('仓库列表', () => {
  beforeEach(async () => {
    localStorage.clear();
    await resetScenario('default');
    await loginAs('admin', 'admin123');
  });

  it('渲染默认场景仓库行', async () => {
    renderWithProviders(<RepositoriesPage />);
    expect(await screen.findByText('raw-hosted')).toBeInTheDocument();
    expect(screen.getByText('maven-releases')).toBeInTheDocument();
  });
});
