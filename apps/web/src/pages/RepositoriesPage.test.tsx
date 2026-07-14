// 仓库列表测试：渲染默认场景仓库行、统计/状态/上游列与操作列、删除确认流程。
import { fireEvent, screen, waitFor, within } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { repoApi } from '../api/endpoints';
import { loginAs, renderWithProviders, resetScenario } from '../test/utils';
import { RepositoriesPage } from './RepositoriesPage';

// 取指定仓库所在的表格行（名称链接向上找 tr）
function repoRow(name: string): HTMLElement {
  const row = screen.getByRole('link', { name }).closest('tr');
  expect(row).not.toBeNull();
  return row as HTMLElement;
}

// jsdom 无真实指针事件：键盘打开 Radix Select 并选中指定选项
async function selectOption(comboboxName: string, optionName: string) {
  fireEvent.keyDown(screen.getByRole('combobox', { name: comboboxName }), { key: 'ArrowDown' });
  const option = await screen.findByRole('option', { name: optionName });
  fireEvent.keyDown(option, { key: 'Enter' });
}

describe('仓库列表', () => {
  beforeEach(async () => {
    localStorage.clear();
    await resetScenario('default');
    await loginAs('admin', 'admin123');
  });

  it('渲染默认场景仓库行', async () => {
    renderWithProviders(<RepositoriesPage />);
    expect(await screen.findByText('raw-hosted')).toBeInTheDocument();
    expect(screen.getByText('maven-releases')).toBeInTheDocument();
  });

  it('渲染制品数/总大小/状态/上游列与操作按钮', async () => {
    renderWithProviders(<RepositoriesPage />);
    expect(await screen.findByText('raw-hosted')).toBeInTheDocument();
    // raw-hosted：3 个种子文件合计 11_691_456 字节
    const row = repoRow('raw-hosted');
    expect(within(row).getByText('3')).toBeInTheDocument();
    expect(within(row).getByText('11.1 MB')).toBeInTheDocument();
    expect(within(row).getByText('启用')).toBeInTheDocument();
    // proxy 仓库展示上游地址，其余为占位符
    const proxyRow = repoRow('npm-proxy');
    expect(within(proxyRow).getByText('https://registry.npmjs.org')).toBeInTheDocument();
    expect(within(row).getByText('—')).toBeInTheDocument();
    // 操作列：设置直达详情配置 tab，删除弹确认对话框
    expect(screen.getByRole('link', { name: '仓库设置 raw-hosted' })).toHaveAttribute(
      'href',
      '/repos/raw-hosted?tab=config',
    );
    expect(screen.getByRole('button', { name: '删除仓库 raw-hosted' })).toBeInTheDocument();
  });

  it('新建 proxy 仓库提交类型专属上游与可选凭据引用', async () => {
    const createSpy = vi.spyOn(repoApi, 'create');
    renderWithProviders(<RepositoriesPage />);
    await screen.findByText('raw-hosted');
    fireEvent.click(screen.getByRole('button', { name: '新建仓库' }));
    fireEvent.change(screen.getByLabelText('仓库名'), { target: { value: 'proxy-demo' } });
    await selectOption('类型', '代理');
    fireEvent.change(screen.getByLabelText('上游地址'), {
      target: { value: 'https://packages.example.com' },
    });
    fireEvent.change(screen.getByLabelText('上游凭据引用'), {
      target: { value: 'EXAMPLE_AUTH' },
    });
    fireEvent.click(screen.getByRole('button', { name: '创建' }));

    await waitFor(() => {
      expect(createSpy).toHaveBeenCalledWith({
        name: 'proxy-demo',
        format: 'raw',
        repo_type: 'proxy',
        visibility: 'private',
        upstream_url: 'https://packages.example.com',
        upstream_credential_ref: 'EXAMPLE_AUTH',
      });
    });
  });

  it('删除对话框须原样输入仓库名才可确认，删除后行消失并提示', async () => {
    renderWithProviders(<RepositoriesPage />);
    expect(await screen.findByText('raw-hosted')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: '删除仓库 raw-hosted' }));
    const confirm = await screen.findByRole('button', { name: '确认删除' });
    // 未输入 / 输错仓库名时确认按钮禁用
    expect(confirm).toBeDisabled();
    const input = screen.getByLabelText('输入仓库名 raw-hosted 以确认');
    fireEvent.change(input, { target: { value: 'raw-hoste' } });
    expect(confirm).toBeDisabled();
    // 原样输入后可确认
    fireEvent.change(input, { target: { value: 'raw-hosted' } });
    expect(confirm).toBeEnabled();
    fireEvent.click(confirm);
    // 删除成功：列表刷新后该行消失，页首提示
    await waitFor(() => {
      expect(screen.queryByRole('link', { name: 'raw-hosted' })).not.toBeInTheDocument();
    });
    expect(screen.getByText('已删除仓库 raw-hosted')).toBeInTheDocument();
  });

  it('按名称筛选后仅保留匹配的仓库行', async () => {
    renderWithProviders(<RepositoriesPage />);
    // 先等全量列表渲染
    expect(await screen.findByText('raw-hosted')).toBeInTheDocument();
    // 输入 maven，防抖后经后端 q 筛选，仅 maven-* 仓库保留
    fireEvent.change(screen.getByPlaceholderText('按名称筛选…'), {
      target: { value: 'maven' },
    });
    // 等筛选结果落定：raw-hosted 消失且 maven-releases 仍在（排除加载骨架的瞬时空窗）
    await waitFor(() => {
      expect(screen.queryByText('raw-hosted')).not.toBeInTheDocument();
      expect(screen.getByText('maven-releases')).toBeInTheDocument();
    });
    expect(screen.getByText('maven-central')).toBeInTheDocument();
    expect(screen.queryByText('npm-proxy')).not.toBeInTheDocument();
  });
});
