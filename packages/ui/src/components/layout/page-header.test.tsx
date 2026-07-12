// PageHeader 页面标题区测试
import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { PageHeader } from './page-header';

describe('PageHeader 页面标题区', () => {
  it('以 h1 渲染标题与描述', () => {
    render(<PageHeader title="仓库列表" description="管理全部仓库" />);
    expect(screen.getByRole('heading', { level: 1, name: '仓库列表' })).toBeInTheDocument();
    expect(screen.getByText('管理全部仓库')).toBeInTheDocument();
  });

  it('渲染操作槽', () => {
    render(<PageHeader title="仓库列表" actions={<button>新建仓库</button>} />);
    expect(screen.getByRole('button', { name: '新建仓库' })).toBeInTheDocument();
  });
});
