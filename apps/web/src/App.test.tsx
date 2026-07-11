// 管理端壳首页渲染测试
import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import App from './App';

describe('管理端壳应用', () => {
  it('渲染出一级标题 JianArtifact', () => {
    render(<App />);
    expect(screen.getByRole('heading', { level: 1, name: 'JianArtifact' })).toBeInTheDocument();
  });
});
