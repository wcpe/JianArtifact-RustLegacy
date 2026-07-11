// 组件验收站首页渲染测试
import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import App from './App';

describe('组件验收站', () => {
  it('渲染出一级标题与至少一个 variant 按钮', () => {
    render(<App />);
    expect(
      screen.getByRole('heading', {
        level: 1,
        name: 'JianArtifact 组件验收站',
      }),
    ).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'default / default' })).toBeInTheDocument();
  });
});
