// EmptyState 空态行为测试
import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { EmptyState } from './empty-state';

describe('EmptyState 空态', () => {
  it('渲染标题与描述', () => {
    render(<EmptyState title="暂无数据" description="尚未创建任何仓库" />);
    expect(screen.getByText('暂无数据')).toBeInTheDocument();
    expect(screen.getByText('尚未创建任何仓库')).toBeInTheDocument();
  });

  it('以 status 语义 role 呈现', () => {
    render(<EmptyState title="暂无数据" />);
    expect(screen.getByRole('status')).toBeInTheDocument();
  });

  it('渲染操作槽内容', () => {
    render(<EmptyState title="暂无数据" action={<button>新建</button>} />);
    expect(screen.getByRole('button', { name: '新建' })).toBeInTheDocument();
  });
});
