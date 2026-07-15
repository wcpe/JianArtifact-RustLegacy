// 仓库接入片段测试：按后端契约展示 Raw/Maven/npm 内容，并验证可访问复制反馈。
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ApiClientError } from '../../api/client';
import { repoApi } from '../../api/endpoints';
import { UsagePanel } from './UsagePanel';

const FORMAT_SNIPPETS = [
  {
    name: 'raw-hosted',
    snippets: [
      {
        title: 'curl 上传',
        content:
          'curl -u <用户名>:<令牌> -T ./文件.bin https://<服务器地址>/repository/raw-hosted/文件.bin',
      },
    ],
  },
  {
    name: 'maven-releases',
    snippets: [
      {
        title: 'pom.xml 仓库配置',
        content:
          '<repository>\n  <id>maven-releases</id>\n  <url>https://<服务器地址>/repository/maven-releases</url>\n</repository>',
      },
      {
        title: 'settings.xml 凭据',
        content:
          '<server>\n  <username><用户名></username>\n  <password><令牌></password>\n</server>',
      },
    ],
  },
  {
    name: 'npm-internal',
    snippets: [
      {
        title: '.npmrc 配置',
        content:
          'registry=https://<服务器地址>/repository/npm-internal/\n//<服务器地址>/repository/npm-internal/:_authToken=<令牌>',
      },
    ],
  },
] as const;

describe('UsagePanel', () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it.each(FORMAT_SNIPPETS)('原样展示 $name 的结构化片段', async ({ name, snippets }) => {
    vi.spyOn(repoApi, 'usage').mockResolvedValue([...snippets]);

    render(<UsagePanel name={name} />);

    for (const snippet of snippets) {
      const heading = await screen.findByRole('heading', { name: snippet.title });
      const section = heading.parentElement?.parentElement;
      if (!section) throw new Error('未找到使用片段容器');
      expect(section.querySelector('pre')).toHaveTextContent(snippet.content, {
        normalizeWhitespace: false,
      });
      expect(
        within(section).getByRole('button', { name: `复制 ${snippet.title}` }),
      ).toBeInTheDocument();
    }
  });

  it('复制成功后以状态消息给出中文反馈', async () => {
    const content = 'curl -u <用户名>:<令牌> https://<服务器地址>/repository/raw-hosted/文件.bin';
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } });
    vi.spyOn(repoApi, 'usage').mockResolvedValue([{ title: 'curl 下载', content }]);
    render(<UsagePanel name="raw-hosted" />);

    fireEvent.click(await screen.findByRole('button', { name: '复制 curl 下载' }));

    await waitFor(() => expect(writeText).toHaveBeenCalledWith(content));
    expect(screen.getByRole('status')).toHaveTextContent('已复制 curl 下载');
  });

  it('剪贴板拒绝时以警告消息给出中文反馈', async () => {
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText: vi.fn().mockRejectedValue(new Error('权限拒绝')) },
    });
    vi.spyOn(repoApi, 'usage').mockResolvedValue([
      { title: '.npmrc 配置', content: 'registry=<地址>' },
    ]);
    render(<UsagePanel name="npm-internal" />);

    fireEvent.click(await screen.findByRole('button', { name: '复制 .npmrc 配置' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('复制 .npmrc 配置失败，请手动复制');
  });

  it('加载中与 403 错误继续使用现有状态组件', async () => {
    let rejectUsage: (reason: ApiClientError) => void = () => undefined;
    vi.spyOn(repoApi, 'usage').mockImplementation(
      () => new Promise((_, reject) => (rejectUsage = reject)),
    );
    const { container } = render(<UsagePanel name="private-repo" />);
    expect(container.querySelector('.animate-pulse')).toBeInTheDocument();

    rejectUsage(new ApiClientError(403, 'FORBIDDEN', '无权查看仓库使用方式'));

    expect(await screen.findByText('无权查看仓库使用方式')).toBeInTheDocument();
  });
});
