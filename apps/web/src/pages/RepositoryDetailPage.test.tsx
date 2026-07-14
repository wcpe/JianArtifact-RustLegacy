// 仓库详情测试：三标签页与 hash 映射、文件浏览（M1-FR-019）、校验和/下载、配置保存与 ACL 增删。
import { fireEvent, screen, within } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { Route, Routes } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { repoApi } from '../api/endpoints';
import { server } from '../test/setup';
import { loginAs, renderWithProviders, resetScenario } from '../test/utils';
import { RepositoryDetailPage } from './RepositoryDetailPage';

// devmock 种子文件 manual.pdf 的伪校验和（fakeHash(1, n)：n-1 个 0 + 1）
const MANUAL_PDF_SHA256 = `${'0'.repeat(63)}1`;
const MANUAL_PDF_SHA1 = `${'0'.repeat(39)}1`;
const MANUAL_PDF_MD5 = `${'0'.repeat(31)}1`;
const MANUAL_PDF_SHA512 = `${'0'.repeat(127)}1`;

// 渲染指定路由下的仓库详情页（缺省 raw-hosted，可带 hash 直达标签页）
function renderDetail(route = '/repos/raw-hosted') {
  renderWithProviders(
    <Routes>
      <Route path="/repos/:name" element={<RepositoryDetailPage />} />
    </Routes>,
    { route },
  );
}

// jsdom 下切换 Radix Tabs：触发器以 mousedown 激活
function clickTab(name: string) {
  fireEvent.mouseDown(screen.getByRole('tab', { name }));
}

// jsdom 无真实指针事件：键盘打开 Radix Select 并以 Enter 选中指定选项
async function selectOption(comboboxName: string, optionName: string) {
  fireEvent.keyDown(screen.getByRole('combobox', { name: comboboxName }), { key: 'ArrowDown' });
  const option = await screen.findByRole('option', { name: optionName });
  fireEvent.keyDown(option, { key: 'Enter' });
}

// 树内展开 docs 目录并选中 manual.pdf 文件
async function selectManualPdf() {
  const tree = await screen.findByRole('tree');
  fireEvent.click(within(tree).getByRole('button', { name: 'docs' }));
  fireEvent.click(await within(tree).findByRole('button', { name: 'manual.pdf' }));
}

describe('仓库详情', () => {
  beforeEach(async () => {
    localStorage.clear();
    // 归一 jsdom 地址：上个用例经 replaceState 写入的 hash 不得泄漏到下个用例
    window.history.replaceState(null, '', '/');
    await resetScenario('default');
    await loginAs('admin', 'admin123');
  });

  it('头部展示返回链接与徽标组，默认选中浏览标签页', async () => {
    renderDetail();
    expect(await screen.findByRole('heading', { name: 'raw-hosted' })).toBeInTheDocument();
    expect(screen.getByRole('link', { name: /返回仓库列表/ })).toHaveAttribute('href', '/repos');
    expect(screen.getByText('RAW')).toBeInTheDocument();
    expect(screen.getByRole('tab', { name: '浏览', selected: true })).toBeInTheDocument();
    // 左侧树与右侧根目录内容
    const tree = await screen.findByRole('tree');
    expect(within(tree).getByRole('button', { name: 'docs' })).toBeInTheDocument();
    expect(within(tree).getByRole('button', { name: 'tools' })).toBeInTheDocument();
    // 默认未选文件：右侧显示选择提示（点目录不再显示目录内容表）
    expect(screen.getByText(/从左侧文件树选择一个文件查看详情/)).toBeInTheDocument();
  });

  it('切换标签页展示对应内容', async () => {
    renderDetail();
    await screen.findByRole('heading', { name: 'raw-hosted' });
    clickTab('配置');
    expect(await screen.findByText('危险操作')).toBeInTheDocument();
    clickTab('浏览');
    expect(await screen.findByRole('tree')).toBeInTheDocument();
  });

  it('直达带 tab 查询参数的地址选中对应标签页', async () => {
    renderDetail('/repos/raw-hosted?tab=acl');
    expect(await screen.findByRole('tab', { name: '权限', selected: true })).toBeInTheDocument();
    expect(await screen.findByText(/暂无授权/)).toBeInTheDocument();
  });

  it('直达带 file 查询参数的地址恢复选中文件详情', async () => {
    // 逐级展开树到 docs/manual.pdf 并在右侧展示其详情（前进/后退与刷新可恢复）
    renderDetail('/repos/raw-hosted?tab=browse&file=docs/manual.pdf');
    expect(await screen.findByRole('heading', { name: 'docs/manual.pdf' })).toBeInTheDocument();
    expect(screen.getByText('application/pdf')).toBeInTheDocument();
  });

  it('文件树直接信任接口返回的完整 path，不按父目录与名称重新拼接', async () => {
    server.use(
      http.get('*/api/v1/repos/raw-hosted/files', ({ request }) => {
        const path = new URL(request.url).searchParams.get('path') ?? '';
        if (path === '') {
          return HttpResponse.json({
            items: [{ name: 'docs', path: 'canonical-docs', is_dir: true }],
            page: 1,
            page_size: 200,
            total: 1,
          });
        }
        if (path === 'canonical-docs') {
          return HttpResponse.json({
            items: [
              {
                name: 'manual.pdf',
                path: 'canonical-docs/manual.pdf',
                is_dir: false,
                size: 1024,
                content_type: 'application/pdf',
              },
            ],
            page: 1,
            page_size: 200,
            total: 1,
          });
        }
        return HttpResponse.json({ code: 'NOT_FOUND', message: '路径不存在' }, { status: 404 });
      }),
    );

    renderDetail();
    const tree = await screen.findByRole('tree');
    fireEvent.click(within(tree).getByRole('button', { name: 'docs' }));
    fireEvent.click(await within(tree).findByRole('button', { name: 'manual.pdf' }));
    expect(
      await screen.findByRole('heading', { name: 'canonical-docs/manual.pdf' }),
    ).toBeInTheDocument();
    expect(screen.getByRole('link', { name: '下载' })).toHaveAttribute(
      'href',
      '/repository/raw-hosted/canonical-docs/manual.pdf',
    );
  });

  it('选中文件后展示元数据、校验和卡与下载按钮', async () => {
    renderDetail();
    await selectManualPdf();
    // 文件路径作为详情标题（raw 无格式坐标卡）
    expect(await screen.findByRole('heading', { name: 'docs/manual.pdf' })).toBeInTheDocument();
    expect(screen.getByText('文件信息')).toBeInTheDocument();
    expect(screen.getByText('application/pdf')).toBeInTheDocument();
    // 校验和卡：四种算法逐行展示且各有复制按钮
    expect(screen.getByText('校验和')).toBeInTheDocument();
    expect(screen.getByText(MANUAL_PDF_SHA256)).toBeInTheDocument();
    expect(screen.getByText(MANUAL_PDF_SHA1)).toBeInTheDocument();
    expect(screen.getByText(MANUAL_PDF_MD5)).toBeInTheDocument();
    expect(screen.getByText(MANUAL_PDF_SHA512)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '复制 SHA-256' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '复制 SHA-512' })).toBeInTheDocument();
    // 下载按钮直连格式端点
    const download = screen.getByRole('link', { name: '下载' });
    expect(download).toHaveAttribute('href', '/repository/raw-hosted/docs/manual.pdf');
    expect(download).toHaveAttribute('download');
  });

  it('Maven 仓库选中构件展示坐标卡与 Gradle KTS 依赖片段', async () => {
    renderDetail('/repos/maven-releases');
    const tree = await screen.findByRole('tree');
    // 逐层展开 com → example → demo → 1.0.0，选中 demo-1.0.0.jar
    fireEvent.click(within(tree).getByRole('button', { name: 'com' }));
    fireEvent.click(await within(tree).findByRole('button', { name: 'example' }));
    fireEvent.click(await within(tree).findByRole('button', { name: 'demo' }));
    fireEvent.click(await within(tree).findByRole('button', { name: '1.0.0' }));
    fireEvent.click(await within(tree).findByRole('button', { name: 'demo-1.0.0.jar' }));
    // 坐标卡：GAV 推导
    expect(await screen.findByText('Maven 坐标')).toBeInTheDocument();
    expect(screen.getByText('com.example')).toBeInTheDocument();
    // 依赖声明默认 Maven 页签，切到 Gradle KTS 得 implementation("…")
    fireEvent.click(screen.getByRole('tab', { name: 'Gradle KTS' }));
    expect(screen.getByText('implementation("com.example:demo:1.0.0")')).toBeInTheDocument();
  });

  it('npm 仓库选中 tarball 展示包卡与 pnpm 安装命令', async () => {
    renderDetail('/repos/npm-internal');
    const tree = await screen.findByRole('tree');
    // 目录结构逐层展开 @acme → ui → - → tarball（与 Maven 一致按路径分段）
    fireEvent.click(within(tree).getByRole('button', { name: '@acme' }));
    fireEvent.click(await within(tree).findByRole('button', { name: 'ui' }));
    fireEvent.click(await within(tree).findByRole('button', { name: '-' }));
    fireEvent.click(await within(tree).findByRole('button', { name: 'ui-1.0.0.tgz' }));
    // 坐标卡按完整路径推导出包名（含 scope）与版本
    expect(await screen.findByText('npm 包')).toBeInTheDocument();
    expect(screen.getByText('@acme/ui')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('tab', { name: 'pnpm' }));
    expect(screen.getByText(/pnpm add @acme\/ui@1\.0\.0 --registry=/)).toBeInTheDocument();
  });

  it('配置标签页保存后提示已保存', async () => {
    renderDetail();
    await screen.findByRole('heading', { name: 'raw-hosted' });
    clickTab('配置');
    fireEvent.click(await screen.findByRole('button', { name: '保存' }));
    expect(await screen.findByText('已保存')).toBeInTheDocument();
  });

  it('非管理员访问权限标签页时按后端 403 展示无权占位', async () => {
    // 以普通用户 dev 登录：raw-hosted 为 public 仅有 read，ACL 端点被后端拒绝
    await loginAs('dev', 'dev123');
    renderDetail('/repos/raw-hosted?tab=acl');
    expect(await screen.findByText('仅仓库管理员可查看与管理访问授权')).toBeInTheDocument();
  });

  it('权限标签页按用户名添加并移除授权', async () => {
    const setSpy = vi.spyOn(repoApi, 'aclSet');
    const removeSpy = vi.spyOn(repoApi, 'aclRemove');
    renderDetail('/repos/raw-hosted?tab=acl');
    expect(await screen.findByText(/暂无授权/)).toBeInTheDocument();
    // 选择用户 dev（默认权限档位 read）后添加
    await selectOption('用户', 'dev');
    fireEvent.click(screen.getByRole('button', { name: '添加' }));
    const table = screen.getByRole('table');
    expect(await within(table).findByText('dev')).toBeInTheDocument();
    expect(setSpy).toHaveBeenCalledWith('raw-hosted', 'dev', 'read');
    expect(within(table).getByText('读')).toBeInTheDocument();
    // 移除后回到空态
    fireEvent.click(within(table).getByRole('button', { name: '移除' }));
    expect(await screen.findByText(/暂无授权/)).toBeInTheDocument();
    expect(removeSpy).toHaveBeenCalledWith('raw-hosted', 'dev');
  });
});
