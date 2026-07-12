// ErrorState 错误态行为测试
import { render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { ErrorState } from './error-state';

describe('ErrorState 错误态', () => {
  it('以 alert 语义渲染错误信息', () => {
    render(<ErrorState message="加载仓库失败" />);
    expect(screen.getByRole('alert')).toBeInTheDocument();
    expect(screen.getByText('加载仓库失败')).toBeInTheDocument();
  });

  it('默认标题为“出错了”', () => {
    render(<ErrorState message="网络异常" />);
    expect(screen.getByText('出错了')).toBeInTheDocument();
  });

  it('提供 onRetry 时点击触发回调', () => {
    const onRetry = vi.fn();
    render(<ErrorState message="失败" onRetry={onRetry} />);
    screen.getByRole('button', { name: '重试' }).click();
    expect(onRetry).toHaveBeenCalledTimes(1);
  });

  it('未提供 onRetry 时不渲染重试按钮', () => {
    render(<ErrorState message="失败" />);
    expect(screen.queryByRole('button', { name: '重试' })).not.toBeInTheDocument();
  });
});
