// Button 组件行为测试：文本渲染、点击回调、禁用态
import { render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { Button } from './button';

describe('Button 组件', () => {
  it('渲染出传入的文本', () => {
    render(<Button>提交</Button>);
    expect(screen.getByRole('button', { name: '提交' })).toBeInTheDocument();
  });

  it('点击时触发 onClick 回调', () => {
    const onClick = vi.fn();
    render(<Button onClick={onClick}>点我</Button>);
    screen.getByRole('button', { name: '点我' }).click();
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  it('禁用时点击不触发回调且带 disabled 属性', () => {
    const onClick = vi.fn();
    render(
      <Button disabled onClick={onClick}>
        禁用
      </Button>,
    );
    const button = screen.getByRole('button', { name: '禁用' });
    button.click();
    expect(onClick).not.toHaveBeenCalled();
    expect(button).toBeDisabled();
  });
});
