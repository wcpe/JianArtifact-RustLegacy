// ForbiddenState 无权限态测试
import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { ForbiddenState } from './forbidden-state';

describe('ForbiddenState 无权限态', () => {
  it('默认渲染“无权访问此资源”', () => {
    render(<ForbiddenState />);
    expect(screen.getByText('无权访问此资源')).toBeInTheDocument();
    expect(screen.getByRole('status')).toBeInTheDocument();
  });

  it('可自定义提示文案', () => {
    render(<ForbiddenState message="仅管理员可见" />);
    expect(screen.getByText('仅管理员可见')).toBeInTheDocument();
  });
});
