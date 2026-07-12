// StatusBadge 状态徽标映射测试
import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { StatusBadge } from './status-badge';

describe('StatusBadge 状态徽标', () => {
  it('可见性映射为中文标签', () => {
    const { rerender } = render(<StatusBadge kind="visibility" value="public" />);
    expect(screen.getByText('公开')).toBeInTheDocument();
    rerender(<StatusBadge kind="visibility" value="private" />);
    expect(screen.getByText('私有')).toBeInTheDocument();
  });

  it('仓库类型映射为中文标签', () => {
    const { rerender } = render(<StatusBadge kind="repo-type" value="hosted" />);
    expect(screen.getByText('宿主')).toBeInTheDocument();
    rerender(<StatusBadge kind="repo-type" value="proxy" />);
    expect(screen.getByText('代理')).toBeInTheDocument();
    rerender(<StatusBadge kind="repo-type" value="group" />);
    expect(screen.getByText('聚合组')).toBeInTheDocument();
  });

  it('任务状态十态全部映射为对应中文', () => {
    const cases: Array<[string, string]> = [
      ['queued', '排队中'],
      ['running', '运行中'],
      ['pausing', '暂停中'],
      ['paused', '已暂停'],
      ['cancelling', '取消中'],
      ['cancelled', '已取消'],
      ['succeeded', '全量成功'],
      ['partial', '部分完成'],
      ['failed', '失败'],
      ['interrupted', '已中断'],
    ];
    for (const [value, label] of cases) {
      const { unmount } = render(<StatusBadge kind="task-status" value={value} />);
      expect(screen.getByText(label)).toBeInTheDocument();
      unmount();
    }
  });

  it('未知值回退为原始文本', () => {
    render(<StatusBadge kind="task-status" value="unknown-x" />);
    expect(screen.getByText('unknown-x')).toBeInTheDocument();
  });
});
