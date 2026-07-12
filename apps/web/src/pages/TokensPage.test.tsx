// API Token 页测试：列表渲染，以及新建后一次性展示明文。
import { fireEvent, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it } from 'vitest';
import { loginAs, renderWithProviders, resetScenario } from '../test/utils';
import { TokensPage } from './TokensPage';

describe('API Token 页', () => {
  beforeEach(async () => {
    localStorage.clear();
    await resetScenario('default');
    await loginAs('admin', 'admin123');
  });

  it('渲染本人 Token 列表', async () => {
    renderWithProviders(<TokensPage />);
    expect(await screen.findByText('ci 发布')).toBeInTheDocument();
  });

  it('新建 Token 后一次性展示明文', async () => {
    renderWithProviders(<TokensPage />);
    // 先等列表加载完成，避免测试结束后仍有未处理请求
    await screen.findByText('ci 发布');
    fireEvent.click(screen.getByRole('button', { name: /新建 Token/ }));
    fireEvent.change(await screen.findByLabelText('Token 名称'), {
      target: { value: '部署令牌' },
    });
    fireEvent.click(screen.getByRole('button', { name: '创建' }));
    // 签发成功后展示明文与「仅此一次」提示
    expect(await screen.findByText(/jat_mock_/)).toBeInTheDocument();
    expect(screen.getByText(/仅此一次/)).toBeInTheDocument();
  });
});
