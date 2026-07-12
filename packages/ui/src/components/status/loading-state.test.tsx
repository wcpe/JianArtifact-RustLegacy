// LoadingState 与 TableSkeleton 加载态测试
import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { LoadingState, TableSkeleton } from './loading-state';

describe('LoadingState 加载态', () => {
  it('渲染 status 语义与默认骨架行数', () => {
    const { container } = render(<LoadingState />);
    expect(screen.getByRole('status')).toBeInTheDocument();
    expect(container.querySelectorAll('[data-slot="skeleton"]')).toHaveLength(3);
  });

  it('按 rows 渲染指定骨架行数', () => {
    const { container } = render(<LoadingState rows={5} />);
    expect(container.querySelectorAll('[data-slot="skeleton"]')).toHaveLength(5);
  });
});

describe('TableSkeleton 表格骨架', () => {
  it('按行列数生成骨架单元格与表头', () => {
    const { container } = render(<TableSkeleton rows={3} columns={2} />);
    // 表头 2 个 + 表体 3×2 个 = 8 个骨架块
    expect(container.querySelectorAll('[data-slot="skeleton"]')).toHaveLength(8);
    expect(screen.getAllByRole('columnheader')).toHaveLength(2);
  });
});
