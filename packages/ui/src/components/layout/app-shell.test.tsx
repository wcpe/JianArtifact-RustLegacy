// AppShell 应用外壳布局测试
import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { AppShell } from './app-shell';

describe('AppShell 应用外壳', () => {
  it('渲染侧栏、顶栏与主内容并具备语义地标', () => {
    render(
      <AppShell sidebar={<nav>导航区</nav>} header={<span>顶栏区</span>}>
        <p>主内容区</p>
      </AppShell>,
    );
    expect(screen.getByRole('complementary', { name: '侧边导航' })).toBeInTheDocument();
    expect(screen.getByRole('banner')).toBeInTheDocument();
    expect(screen.getByRole('main')).toBeInTheDocument();
    expect(screen.getByText('主内容区')).toBeInTheDocument();
  });

  it('未提供 header 时不渲染顶栏', () => {
    render(
      <AppShell sidebar={<nav>导航区</nav>}>
        <p>主内容区</p>
      </AppShell>,
    );
    expect(screen.queryByRole('banner')).not.toBeInTheDocument();
  });
});
