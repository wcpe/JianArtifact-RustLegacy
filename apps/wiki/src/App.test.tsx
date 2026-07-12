// 组件验收站测试：总览索引、导航切换与状态矩阵渲染
import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import App from './App';

describe('组件验收站', () => {
  it('总览含一级标题与组件索引', () => {
    render(<App />);
    expect(
      screen.getByRole('heading', { level: 1, name: 'JianArtifact 组件验收站' }),
    ).toBeInTheDocument();
    // 索引列表里应能看到复合层与状态层代表组件
    expect(screen.getByRole('button', { name: 'DataTable' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'StatusBadge' })).toBeInTheDocument();
    expect(screen.getByText('状态覆盖说明')).toBeInTheDocument();
  });

  it('切换到 StatusBadge 展示任务十态映射', () => {
    render(<App />);
    fireEvent.click(screen.getByRole('button', { name: 'StatusBadge' }));
    // 十态中文映射的代表值应可见
    expect(screen.getByText('全量成功')).toBeInTheDocument();
    expect(screen.getByText('已中断')).toBeInTheDocument();
  });

  it('切换到 DataTable 展示加载与大数据量状态块', () => {
    render(<App />);
    fireEvent.click(screen.getByRole('button', { name: 'DataTable' }));
    const blocks = screen.getAllByText(/DataTable · /);
    expect(blocks.length).toBeGreaterThanOrEqual(2);
  });

  it('导航选中项带 aria-current', () => {
    render(<App />);
    const nav = screen.getByRole('button', { name: 'EmptyState' });
    fireEvent.click(nav);
    expect(nav).toHaveAttribute('aria-current', 'page');
  });
});
