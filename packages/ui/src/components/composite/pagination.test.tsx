// Pagination 分页交互与边界测试
import { render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { Pagination } from './pagination';

describe('Pagination 分页', () => {
  it('展示页码信息', () => {
    render(<Pagination page={2} pageSize={20} total={130} onPageChange={vi.fn()} />);
    // 130 条、每页 20 → 共 7 页
    expect(screen.getByText('第 2 / 7 页，共 130 条')).toBeInTheDocument();
  });

  it('首页禁用上一页、启用下一页', () => {
    render(<Pagination page={1} pageSize={20} total={130} onPageChange={vi.fn()} />);
    expect(screen.getByRole('button', { name: '上一页' })).toBeDisabled();
    expect(screen.getByRole('button', { name: '下一页' })).toBeEnabled();
  });

  it('末页禁用下一页、启用上一页', () => {
    render(<Pagination page={7} pageSize={20} total={130} onPageChange={vi.fn()} />);
    expect(screen.getByRole('button', { name: '下一页' })).toBeDisabled();
    expect(screen.getByRole('button', { name: '上一页' })).toBeEnabled();
  });

  it('翻页触发回调并带正确页码', () => {
    const onPageChange = vi.fn();
    render(<Pagination page={3} pageSize={20} total={130} onPageChange={onPageChange} />);
    screen.getByRole('button', { name: '上一页' }).click();
    screen.getByRole('button', { name: '下一页' }).click();
    expect(onPageChange).toHaveBeenNthCalledWith(1, 2);
    expect(onPageChange).toHaveBeenNthCalledWith(2, 4);
  });

  it('总数为 0 时仍显示 1 页且两侧按钮均禁用', () => {
    render(<Pagination page={1} pageSize={20} total={0} onPageChange={vi.fn()} />);
    expect(screen.getByText('第 1 / 1 页，共 0 条')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '上一页' })).toBeDisabled();
    expect(screen.getByRole('button', { name: '下一页' })).toBeDisabled();
  });
});
