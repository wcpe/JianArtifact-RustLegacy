// 设置页组件测试（FR-87 只读 + FR-88 可编辑热替换 + FR-106 动态配置 + FR-129 顶部 Tab 分页 + FR-146 单槽代理）：
// 加载填充脱敏配置到表单、单个全局保存一次性提交 settings + dynamic + protection 三次 PATCH、
// 单一通用代理三字段 / 密码三态契约（FR-100 + FR-146）、动态配置各节回显与字段绑定、各错误码（400）友好提示；
// 并校验顶部 Tab 分页（六项，点 Tab 切换显示对应节内容、非默认节切换前不可见）+ **只有一个保存按钮**。
// FR-129：原锚点长滚动 + scroll-spy 改为 Tab 分页（切换不滚动、根因消除高亮 / hover 错位）；
// 防护节并入单一保存（移除其独立「保存并即时生效」按钮）。
// 注：在线更新已迁至「系统」页（FR-109，SystemPage），本页不再含相关 UI 与测试。

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, waitFor, fireEvent, within, act } from '@testing-library/react';
import { MantineProvider } from '@mantine/core';
import { SettingsPage } from './SettingsPage';
import * as api from '../api/endpoints';
import { ApiError } from '../api/client';
import type { SettingsView, DynamicConfig, ProtectionConfig } from '../api/types';

/** 在 Mantine Provider 下渲染设置页。 */
function renderPage() {
  return render(
    <MantineProvider>
      <SettingsPage />
    </MantineProvider>,
  );
}

/** 点击顶部 Tab 切到指定分节（Tab 标题与各节标题同文案）。 */
function 切到Tab(name: string) {
  const tablist = screen.getByRole('tablist');
  fireEvent.click(within(tablist).getByRole('tab', { name }));
}

/** 构造可由测试显式完成的延迟 Promise。 */
function 延迟结果<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

/** 一份默认动态配置样例（FR-106）。 */
const 动态配置样例: DynamicConfig = {
  limits: { max_artifact_size: null },
  audit: { retention_days: 90, max_rows: 1000000 },
  usage: { detail_enabled: false, max_detail_rows: 1000000 },
  metrics: { enabled: true, allow_anonymous: false },
  metrics_timeseries: {
    enabled: true,
    sample_interval_secs: 60,
    retention_days: 7,
    max_rows: 1000000,
  },
  vuln: {
    enabled: false,
    source_base_url: 'https://osv.example',
    ecosystems: [],
    refresh_interval_secs: 86400,
    download_timeout_secs: 600,
  },
  auth: { session_ttl_secs: 3600, login_max_failures: 5, login_lockout_secs: 900 },
};

/** 一份启用在线更新、含脱敏代理的设置样例（FR-146 单槽）。 */
const 启用样例: SettingsView = {
  current_version: '0.3.0',
  network_proxy: {
    proxy: { url: 'http://proxy.internal:8080', username: 'alice', has_password: true },
    no_proxy: 'localhost,127.0.0.1',
  },
  update: {
    enabled: true,
    repo: 'wcpe/JianArtifact',
    api_base_url: 'https://api.github.com',
    restart_mode: 'self',
    channel: 'stable',
    has_token: true,
    rollback_available: true,
  },
};

/** 一份未启用在线更新、无代理凭据的设置样例（FR-146 单槽）。 */
const 未启用样例: SettingsView = {
  current_version: '0.3.0',
  network_proxy: {
    proxy: { url: null, username: null, has_password: false },
    no_proxy: null,
  },
  update: {
    enabled: false,
    repo: 'wcpe/JianArtifact',
    api_base_url: 'https://api.github.com',
    restart_mode: 'self',
    channel: 'stable',
    has_token: false,
    rollback_available: false,
  },
};

/** 一份默认（各防护关闭）的防护配置样例（FR-110 防护节嵌入设置页后，页面会加载它）。
 *  各数值刻意避开动态配置样例的 60 / 90 / 3600 等，免得 getByDisplayValue 跨节误命中多个元素。 */
const 防护配置样例: ProtectionConfig = {
  rate_limit: {
    enabled: false,
    window_secs: 61,
    ip_max_requests: 1200,
    identity_max_requests: 2400,
    repo_max_requests: 0,
    ip_max_concurrent: 0,
    user_max_concurrent: 0,
    repo_max_concurrent: 0,
  },
  ip_list: { allow: [], deny: [] },
  ban: { enabled: false, window_secs: 62, threshold: 100, duration_secs: 901 },
  slowloris: {
    enabled: false,
    body_read_timeout_secs: 31,
    header_timeout_secs: 32,
    max_body_bytes: 0,
  },
  cc_challenge: { enabled: false, difficulty: 21, ttl_secs: 301, exempt_authenticated: true },
  waf: { enabled: false, rules: [] },
  alerts: {
    enabled: false,
    window_secs: 301,
    rate_limit_warn_threshold: 1001,
    ban_warn_threshold: 51,
    cc_challenge_fail_warn_threshold: 1002,
    waf_block_warn_threshold: 501,
    slowloris_warn_threshold: 201,
    max_rows: 100001,
  },
};

/** 默认对 getDynamicConfig 打桩（多数用例不关心动态配置，但页面会加载它）。 */
function 桩动态配置(config: DynamicConfig = 动态配置样例) {
  return vi.spyOn(api, 'getDynamicConfig').mockResolvedValue(config);
}

/** 默认对 getProtectionConfig 打桩（FR-110 防护节会在挂载时加载防护配置）。 */
function 桩防护配置(config: ProtectionConfig = 防护配置样例) {
  return vi.spyOn(api, 'getProtectionConfig').mockResolvedValue(config);
}

describe('SettingsPage', () => {
  // FR-110：防护节挂载即拉防护配置；各用例默认打桩，避免真实 fetch。需要定制的用例可再行覆盖。
  beforeEach(() => {
    桩防护配置();
  });
  afterEach(() => vi.restoreAllMocks());

  it('加载后将脱敏后的网络代理配置填入可编辑表单', async () => {
    vi.spyOn(api, 'getSettings').mockResolvedValue(启用样例);
    桩动态配置();
    renderPage();

    await waitFor(() => expect(screen.getByText('通用网络代理')).toBeInTheDocument());
    // 代理 URL 脱敏后填入输入框（不含凭据）
    expect(screen.getByDisplayValue('http://proxy.internal:8080')).toBeInTheDocument();
    expect(screen.getByDisplayValue('localhost,127.0.0.1')).toBeInTheDocument();
  });

  // ===== FR-129：顶部 Tab 分页骨架 =====

  it('FR-129：顶部 Tab 列表有六项（网络代理 / 限制与配额 / 可观测性 / 漏洞库 / 安全·会话 / 防护配置）', async () => {
    vi.spyOn(api, 'getSettings').mockResolvedValue(启用样例);
    桩动态配置();
    renderPage();

    await waitFor(() => expect(screen.getByText('通用网络代理')).toBeInTheDocument());
    const tablist = screen.getByRole('tablist');
    expect(within(tablist).getByRole('tab', { name: '网络代理' })).toBeInTheDocument();
    expect(within(tablist).getByRole('tab', { name: '限制与配额' })).toBeInTheDocument();
    expect(within(tablist).getByRole('tab', { name: '可观测性' })).toBeInTheDocument();
    expect(within(tablist).getByRole('tab', { name: '漏洞库' })).toBeInTheDocument();
    expect(within(tablist).getByRole('tab', { name: '安全 / 会话' })).toBeInTheDocument();
    expect(within(tablist).getByRole('tab', { name: '防护配置' })).toBeInTheDocument();
  });

  it('FR-129：默认显示网络代理节，其余节内容切 Tab 前不可见', async () => {
    vi.spyOn(api, 'getSettings').mockResolvedValue(启用样例);
    桩动态配置();
    renderPage();

    // 默认 Tab 为网络代理：HTTP 代理可见
    await waitFor(() => expect(screen.getByText('通用网络代理')).toBeVisible());
    // 安全 / 会话节字段（会话有效期）虽随面板挂载在 DOM，但未激活面板不可见
    expect(screen.getByLabelText('会话有效期（秒）')).not.toBeVisible();
  });

  it('FR-129：点击 Tab 切换显示对应节内容', async () => {
    vi.spyOn(api, 'getSettings').mockResolvedValue(启用样例);
    桩动态配置();
    renderPage();

    await waitFor(() => expect(screen.getByText('通用网络代理')).toBeVisible());
    // 切到「安全 / 会话」Tab → 会话有效期字段可见
    切到Tab('安全 / 会话');
    await waitFor(() => expect(screen.getByLabelText('会话有效期（秒）')).toBeVisible());
    // 切走后网络代理节内容重新可见、安全会话节内容隐藏
    切到Tab('网络代理');
    await waitFor(() => expect(screen.getByText('通用网络代理')).toBeVisible());
    expect(screen.getByLabelText('会话有效期（秒）')).not.toBeVisible();
  });

  it('FR-129：只有一个保存按钮——保存条内有「保存」、且无防护节「保存并即时生效」按钮', async () => {
    vi.spyOn(api, 'getSettings').mockResolvedValue(启用样例);
    桩动态配置();
    renderPage();

    await waitFor(() => expect(screen.getByText('通用网络代理')).toBeInTheDocument());
    // 保存条在场且 sticky 贴底，内含唯一「保存」按钮
    const saveBar = screen.getByTestId('settings-save-bar');
    expect(saveBar).toHaveStyle({ position: 'sticky', bottom: '0' });
    expect(within(saveBar).getByRole('button', { name: '保存' })).toBeInTheDocument();
    // 旧的「保存系统配置」按钮已去除：整页不存在
    expect(screen.queryByRole('button', { name: '保存系统配置' })).not.toBeInTheDocument();
    // 防护节独立保存按钮已并入全局保存：切到防护 Tab 也不应再出现
    切到Tab('防护配置');
    await waitFor(() => expect(screen.getByText('速率限制')).toBeVisible());
    expect(screen.queryByRole('button', { name: '保存并即时生效' })).not.toBeInTheDocument();
    // 整页文案恰为「保存」的按钮只此一个
    const allButtons = screen.getAllByRole('button');
    const saveButtons = allButtons.filter((b) => b.textContent?.trim() === '保存');
    expect(saveButtons).toHaveLength(1);
  });

  // ===== FR-103/129：单个全局保存（合并 settings + dynamic + protection 三次 PATCH）=====

  it('FR-129：点保存一次性提交 updateSettings + updateDynamicConfig + updateProtectionConfig，成功显已保存', async () => {
    vi.spyOn(api, 'getSettings').mockResolvedValue(未启用样例);
    桩动态配置();
    const update = vi.spyOn(api, 'updateSettings').mockResolvedValue(未启用样例);
    const updateDyn = vi
      .spyOn(api, 'updateDynamicConfig')
      .mockImplementation((c) => Promise.resolve(c));
    const updateProtection = vi
      .spyOn(api, 'updateProtectionConfig')
      .mockImplementation((c) => Promise.resolve(c));
    renderPage();

    await waitFor(() => expect(screen.getByText('通用网络代理')).toBeInTheDocument());
    fireEvent.click(screen.getByRole('button', { name: '保存' }));

    await waitFor(() => expect(update).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(updateDyn).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(updateProtection).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(screen.getByText(/已保存/)).toBeInTheDocument());
  });

  it('FR-129：保存时改过的动态配置（会话有效期）随 updateDynamicConfig 一并提交', async () => {
    vi.spyOn(api, 'getSettings').mockResolvedValue(启用样例);
    桩动态配置();
    vi.spyOn(api, 'updateSettings').mockResolvedValue(启用样例);
    vi.spyOn(api, 'updateProtectionConfig').mockImplementation((c) => Promise.resolve(c));
    const updateDyn = vi
      .spyOn(api, 'updateDynamicConfig')
      .mockImplementation((c) => Promise.resolve(c));
    renderPage();

    await waitFor(() => expect(screen.getByText('通用网络代理')).toBeInTheDocument());
    切到Tab('安全 / 会话');
    await waitFor(() => expect(screen.getByLabelText('会话有效期（秒）')).toBeVisible());
    fireEvent.change(screen.getByLabelText('会话有效期（秒）'), { target: { value: '7200' } });
    fireEvent.click(screen.getByRole('button', { name: '保存' }));

    await waitFor(() => expect(updateDyn).toHaveBeenCalledTimes(1));
    expect(updateDyn.mock.calls[0][0].auth.session_ttl_secs).toBe(7200);
  });

  it('FR-129：保存时改过的防护配置随 updateProtectionConfig 一并提交（防护并入单一保存）', async () => {
    vi.spyOn(api, 'getSettings').mockResolvedValue(启用样例);
    桩动态配置();
    vi.spyOn(api, 'updateSettings').mockResolvedValue(启用样例);
    vi.spyOn(api, 'updateDynamicConfig').mockImplementation((c) => Promise.resolve(c));
    const updateProtection = vi
      .spyOn(api, 'updateProtectionConfig')
      .mockImplementation((c) => Promise.resolve(c));
    renderPage();

    await waitFor(() => expect(screen.getByText('通用网络代理')).toBeInTheDocument());
    // 切到防护 Tab，开启「速率限制」开关
    切到Tab('防护配置');
    await waitFor(() => expect(screen.getByText('速率限制')).toBeVisible());
    fireEvent.click(screen.getByLabelText('启用速率限制'));
    fireEvent.click(screen.getByRole('button', { name: '保存' }));

    await waitFor(() => expect(updateProtection).toHaveBeenCalledTimes(1));
    expect(updateProtection.mock.calls[0][0].rate_limit.enabled).toBe(true);
  });

  it('FR-129：动态配置未加载（getDynamicConfig 失败）时保存只发 settings + protection，不报错', async () => {
    vi.spyOn(api, 'getSettings').mockResolvedValue(启用样例);
    vi.spyOn(api, 'getDynamicConfig').mockRejectedValue(
      new ApiError(403, 'forbidden', '无权执行该操作'),
    );
    const update = vi.spyOn(api, 'updateSettings').mockResolvedValue(启用样例);
    const updateDyn = vi.spyOn(api, 'updateDynamicConfig');
    const updateProtection = vi
      .spyOn(api, 'updateProtectionConfig')
      .mockImplementation((c) => Promise.resolve(c));
    renderPage();

    await waitFor(() => expect(screen.getByText('通用网络代理')).toBeInTheDocument());
    fireEvent.click(screen.getByRole('button', { name: '保存' }));

    await waitFor(() => expect(update).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(updateProtection).toHaveBeenCalledTimes(1));
    expect(updateDyn).not.toHaveBeenCalled();
  });

  it('FR-129：防护配置未加载（getProtectionConfig 失败）时保存只发 settings + dynamic，不报错', async () => {
    vi.spyOn(api, 'getSettings').mockResolvedValue(启用样例);
    桩动态配置();
    vi.spyOn(api, 'getProtectionConfig').mockRejectedValue(
      new ApiError(403, 'forbidden', '无权执行该操作'),
    );
    const update = vi.spyOn(api, 'updateSettings').mockResolvedValue(启用样例);
    const updateDyn = vi
      .spyOn(api, 'updateDynamicConfig')
      .mockImplementation((c) => Promise.resolve(c));
    const updateProtection = vi.spyOn(api, 'updateProtectionConfig');
    renderPage();

    await waitFor(() => expect(screen.getByText('通用网络代理')).toBeInTheDocument());
    fireEvent.click(screen.getByRole('button', { name: '保存' }));

    await waitFor(() => expect(update).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(updateDyn).toHaveBeenCalledTimes(1));
    expect(updateProtection).not.toHaveBeenCalled();
  });

  it('FR-129：保存（settings PATCH）返回 400 时展示友好提示', async () => {
    vi.spyOn(api, 'getSettings').mockResolvedValue(启用样例);
    桩动态配置();
    vi.spyOn(api, 'updateSettings').mockRejectedValue(
      new ApiError(400, 'bad_request', '网络代理配置非法：出站 HTTP 代理配置无效'),
    );
    renderPage();

    await waitFor(() => expect(screen.getByText('通用网络代理')).toBeInTheDocument());
    fireEvent.click(screen.getByRole('button', { name: '保存' }));
    await waitFor(() =>
      expect(screen.getByText('网络代理配置非法：出站 HTTP 代理配置无效')).toBeInTheDocument(),
    );
  });

  it('FR-129：动态配置保存返回 400（非法值）时展示友好提示', async () => {
    vi.spyOn(api, 'getSettings').mockResolvedValue(启用样例);
    桩动态配置();
    vi.spyOn(api, 'updateSettings').mockResolvedValue(启用样例);
    vi.spyOn(api, 'updateDynamicConfig').mockRejectedValue(
      new ApiError(
        400,
        'bad_request',
        '动态配置非法：会话有效期（auth.session_ttl_secs）必须大于 0',
      ),
    );
    renderPage();

    await waitFor(() => expect(screen.getByText('通用网络代理')).toBeInTheDocument());
    fireEvent.click(screen.getByRole('button', { name: '保存' }));

    await waitFor(() =>
      expect(
        screen.getByText('动态配置非法：会话有效期（auth.session_ttl_secs）必须大于 0'),
      ).toBeInTheDocument(),
    );
  });

  it('FR-129：防护配置保存返回 400（非法值）时展示友好提示', async () => {
    vi.spyOn(api, 'getSettings').mockResolvedValue(启用样例);
    桩动态配置();
    vi.spyOn(api, 'updateSettings').mockResolvedValue(启用样例);
    vi.spyOn(api, 'updateDynamicConfig').mockImplementation((c) => Promise.resolve(c));
    vi.spyOn(api, 'updateProtectionConfig').mockRejectedValue(
      new ApiError(400, 'bad_request', '防护配置非法：限流窗口必须大于 0'),
    );
    renderPage();

    await waitFor(() => expect(screen.getByText('通用网络代理')).toBeInTheDocument());
    fireEvent.click(screen.getByRole('button', { name: '保存' }));

    await waitFor(() =>
      expect(screen.getByText('防护配置非法：限流窗口必须大于 0')).toBeInTheDocument(),
    );
  });

  // ===== FR-100 + FR-146：单一通用代理三字段 / 三态（合并到全局保存，契约不回归）=====

  it('编辑代理 URL 后点保存，PATCH 载荷带新 URL、省略 password', async () => {
    vi.spyOn(api, 'getSettings').mockResolvedValue(未启用样例);
    桩动态配置();
    vi.spyOn(api, 'updateProtectionConfig').mockImplementation((c) => Promise.resolve(c));
    const update = vi.spyOn(api, 'updateSettings').mockResolvedValue(未启用样例);
    renderPage();

    await waitFor(() => expect(screen.getByText('通用网络代理')).toBeInTheDocument());
    fireEvent.change(screen.getByLabelText('URL'), {
      target: { value: 'http://new-proxy.internal:3128' },
    });
    fireEvent.click(screen.getByRole('button', { name: '保存' }));

    await waitFor(() => expect(update).toHaveBeenCalledTimes(1));
    const payload = update.mock.calls[0][0];
    expect(payload.network_proxy).toBeDefined();
    expect(payload.network_proxy!.proxy.url).toBe('http://new-proxy.internal:3128');
    expect(payload.network_proxy!.proxy.password).toBeUndefined();
  });

  it('FR-146：单一通用代理槽渲染 URL / 用户名 / 密码三字段与 scheme 提示', async () => {
    vi.spyOn(api, 'getSettings').mockResolvedValue(启用样例);
    桩动态配置();
    renderPage();

    await waitFor(() => expect(screen.getByText('通用网络代理')).toBeInTheDocument());
    // 只有一个代理槽位：三字段各恰一组，旧三槽标题不复存在
    expect(screen.getAllByLabelText('URL')).toHaveLength(1);
    expect(screen.getAllByLabelText('用户名')).toHaveLength(1);
    expect(screen.getAllByLabelText('密码')).toHaveLength(1);
    expect(screen.queryByText('HTTP 代理')).not.toBeInTheDocument();
    expect(screen.queryByText('HTTPS 代理')).not.toBeInTheDocument();
    expect(screen.queryByText(/SOCKS5 代理/)).not.toBeInTheDocument();
    // scheme 支持说明（socks5h 远端 DNS 提示）
    expect(screen.getByText(/socks5h/)).toBeInTheDocument();
  });

  it('FR-100：has_password 为真时展示「密码已配置」徽标', async () => {
    vi.spyOn(api, 'getSettings').mockResolvedValue(启用样例);
    桩动态配置();
    renderPage();

    await waitFor(() => expect(screen.getByText('通用网络代理')).toBeInTheDocument());
    expect(screen.getByText('密码已配置')).toBeInTheDocument();
  });

  it('FR-100：密码框留空保存时 PATCH 载荷省略 password（保留现有）', async () => {
    vi.spyOn(api, 'getSettings').mockResolvedValue(启用样例);
    桩动态配置();
    vi.spyOn(api, 'updateProtectionConfig').mockImplementation((c) => Promise.resolve(c));
    vi.spyOn(api, 'updateDynamicConfig').mockImplementation((c) => Promise.resolve(c));
    const update = vi.spyOn(api, 'updateSettings').mockResolvedValue(启用样例);
    renderPage();

    await waitFor(() => expect(screen.getByText('通用网络代理')).toBeInTheDocument());
    fireEvent.click(screen.getByRole('button', { name: '保存' }));

    await waitFor(() => expect(update).toHaveBeenCalledTimes(1));
    const np = update.mock.calls[0][0].network_proxy!;
    expect(np.proxy.password).toBeUndefined();
    expect(np.proxy.username).toBe('alice');
  });

  it('FR-100：填入密码保存时 PATCH 载荷带 password（设置新密码）', async () => {
    vi.spyOn(api, 'getSettings').mockResolvedValue(启用样例);
    桩动态配置();
    vi.spyOn(api, 'updateProtectionConfig').mockImplementation((c) => Promise.resolve(c));
    vi.spyOn(api, 'updateDynamicConfig').mockImplementation((c) => Promise.resolve(c));
    const update = vi.spyOn(api, 'updateSettings').mockResolvedValue(启用样例);
    renderPage();

    await waitFor(() => expect(screen.getByText('通用网络代理')).toBeInTheDocument());
    fireEvent.change(screen.getByLabelText('密码'), { target: { value: 's3cret' } });
    fireEvent.click(screen.getByRole('button', { name: '保存' }));

    await waitFor(() => expect(update).toHaveBeenCalledTimes(1));
    const np = update.mock.calls[0][0].network_proxy!;
    expect(np.proxy.password).toBe('s3cret');
  });

  it('FR-146：URL 可填 socks5 地址并组装到 proxy 槽', async () => {
    vi.spyOn(api, 'getSettings').mockResolvedValue(未启用样例);
    桩动态配置();
    vi.spyOn(api, 'updateProtectionConfig').mockImplementation((c) => Promise.resolve(c));
    const update = vi.spyOn(api, 'updateSettings').mockResolvedValue(未启用样例);
    renderPage();

    await waitFor(() => expect(screen.getByText('通用网络代理')).toBeInTheDocument());
    fireEvent.change(screen.getByLabelText('URL'), {
      target: { value: 'socks5://socks.internal:1080' },
    });
    fireEvent.click(screen.getByRole('button', { name: '保存' }));

    await waitFor(() => expect(update).toHaveBeenCalledTimes(1));
    const payload = update.mock.calls[0][0];
    expect(payload.network_proxy).toBeDefined();
    expect(payload.network_proxy!.proxy.url).toBe('socks5://socks.internal:1080');
  });

  it('FR-100：点「清除密码」后保存，PATCH 载荷带 password 空串（清空）', async () => {
    vi.spyOn(api, 'getSettings').mockResolvedValue(启用样例);
    桩动态配置();
    vi.spyOn(api, 'updateProtectionConfig').mockImplementation((c) => Promise.resolve(c));
    vi.spyOn(api, 'updateDynamicConfig').mockImplementation((c) => Promise.resolve(c));
    const update = vi.spyOn(api, 'updateSettings').mockResolvedValue(启用样例);
    renderPage();

    await waitFor(() => expect(screen.getByText('通用网络代理')).toBeInTheDocument());
    fireEvent.click(screen.getByText('清除密码'));
    fireEvent.click(screen.getByRole('button', { name: '保存' }));

    await waitFor(() => expect(update).toHaveBeenCalledTimes(1));
    const payload = update.mock.calls[0][0];
    expect(payload.network_proxy).toBeDefined();
    expect(payload.network_proxy!.proxy.password).toBe('');
  });

  it('F-01：清除密码后输入新密码会取消清除态并隐藏清除徽标', async () => {
    vi.spyOn(api, 'getSettings').mockResolvedValue(启用样例);
    桩动态配置();
    renderPage();

    await waitFor(() => expect(screen.getByText('通用网络代理')).toBeInTheDocument());
    fireEvent.click(screen.getByRole('button', { name: '清除密码' }));
    expect(screen.getByText('保存后清除密码')).toBeInTheDocument();

    fireEvent.change(screen.getByLabelText('密码'), { target: { value: 'replacement-secret' } });

    expect(screen.queryByText('保存后清除密码')).not.toBeInTheDocument();
  });

  it('加载失败时展示错误提示', async () => {
    vi.spyOn(api, 'getSettings').mockRejectedValue(
      new ApiError(403, 'forbidden', '无权执行该操作'),
    );
    桩动态配置();
    renderPage();
    await waitFor(() => expect(screen.getByText('无权执行该操作')).toBeInTheDocument());
  });

  // ===== FR-106：动态配置各节并入 Tab 分页（表单回显 + 字段绑定不回归）=====

  it('FR-106：切到各 Tab 后限制配额 / 可观测性 / 安全会话节表单回显默认值', async () => {
    vi.spyOn(api, 'getSettings').mockResolvedValue(启用样例);
    桩动态配置();
    renderPage();

    await waitFor(() => expect(screen.getByText('通用网络代理')).toBeVisible());
    // 可观测性 Tab：审计保留 90、采样间隔 60
    切到Tab('可观测性');
    await waitFor(() => expect(screen.getByDisplayValue('90')).toBeVisible());
    expect(screen.getByDisplayValue('60')).toBeVisible();
    // 安全 / 会话 Tab：会话有效期 3600 + 重启生效徽标（区别于代理即时生效）
    切到Tab('安全 / 会话');
    await waitFor(() => expect(screen.getByDisplayValue('3600')).toBeVisible());
    // 安全会话节内的重启徽标随该 Tab 激活而可见（面板保持挂载，故各节均有徽标，断言激活节的可见）
    const authPanel = screen.getByLabelText('会话有效期（秒）').closest('[role="tabpanel"]');
    expect(authPanel).not.toBeNull();
    expect(within(authPanel as HTMLElement).getByText('保存后重启生效')).toBeVisible();
  });

  // ===== FR-110/129：防护配置并入设置页（Tab 分节 + 并入单一保存）=====

  it('FR-129：顶部 Tab 含「防护配置」一项', async () => {
    vi.spyOn(api, 'getSettings').mockResolvedValue(启用样例);
    桩动态配置();
    renderPage();

    await waitFor(() => expect(screen.getByText('通用网络代理')).toBeInTheDocument());
    const tablist = screen.getByRole('tablist');
    expect(within(tablist).getByRole('tab', { name: '防护配置' })).toBeInTheDocument();
  });

  it('FR-129：切到防护 Tab——防护各维度标题可见、无独立保存按钮', async () => {
    vi.spyOn(api, 'getSettings').mockResolvedValue(启用样例);
    桩动态配置();
    renderPage();

    await waitFor(() => expect(screen.getByText('通用网络代理')).toBeInTheDocument());
    切到Tab('防护配置');
    // 防护各维度分区可见
    await waitFor(() => expect(screen.getByText('速率限制')).toBeVisible());
    expect(screen.getByText('WAF 规则引擎')).toBeVisible();
    // 防护节不再有自己的保存按钮（并入单一保存）
    expect(screen.queryByRole('button', { name: '保存并即时生效' })).not.toBeInTheDocument();
  });

  // ===== FR-128：代理连通性测试（单槽「测试」按钮 → 模态框，FR-146 收敛）=====

  it('FR-128：代理 Tab 有唯一「测试」按钮（单槽）', async () => {
    vi.spyOn(api, 'getSettings').mockResolvedValue(启用样例);
    桩动态配置();
    renderPage();

    await waitFor(() => expect(screen.getByText('通用网络代理')).toBeInTheDocument());
    expect(screen.getByTestId('proxy-test-open')).toBeInTheDocument();
    expect(screen.getByTestId('proxy-test-open')).toHaveTextContent('测试');
    // 旧三槽测试按钮不复存在
    expect(screen.queryByTestId('proxy-test-button-http')).not.toBeInTheDocument();
    expect(screen.queryByTestId('proxy-test-button-https')).not.toBeInTheDocument();
    expect(screen.queryByTestId('proxy-test-button-all')).not.toBeInTheDocument();
  });

  it('FR-146：点「测试」按钮弹出模态框，预填目标 URL 占位值而非代理地址', async () => {
    vi.spyOn(api, 'getSettings').mockResolvedValue(启用样例);
    桩动态配置();
    renderPage();

    await waitFor(() => expect(screen.getByText('通用网络代理')).toBeInTheDocument());
    fireEvent.click(screen.getByTestId('proxy-test-open'));

    // 模态框打开：URL 输入框与内部测试按钮可见
    await waitFor(() => expect(screen.getByTestId('proxy-test-url-input')).toBeInTheDocument());
    // 预填 testUrlPlaceholder 值；**不预填代理地址**（旧行为是已确认缺陷）
    expect((screen.getByTestId('proxy-test-url-input') as HTMLInputElement).value).toBe(
      'https://www.google.com',
    );
    expect(screen.getByTestId('proxy-test-button')).toBeInTheDocument();
  });

  it('FR-128：模态框内点「测试」调 testProxy 并展示连通成功结果', async () => {
    vi.spyOn(api, 'getSettings').mockResolvedValue(启用样例);
    桩动态配置();
    const testProxy = vi.spyOn(api, 'testProxy').mockResolvedValue({
      ok: true,
      status: 200,
      elapsed_ms: 123,
    });
    renderPage();

    await waitFor(() => expect(screen.getByText('通用网络代理')).toBeInTheDocument());
    // 打开测试模态框
    fireEvent.click(screen.getByTestId('proxy-test-open'));
    await waitFor(() => expect(screen.getByTestId('proxy-test-url-input')).toBeInTheDocument());

    // 修改 URL 后点测试
    fireEvent.change(screen.getByTestId('proxy-test-url-input'), {
      target: { value: 'https://example.com' },
    });
    fireEvent.click(screen.getByTestId('proxy-test-button'));

    await waitFor(() => expect(testProxy).toHaveBeenCalledTimes(1));
    const [, options] = testProxy.mock.calls[0];
    expect(options?.proxy).toEqual({ url: 'http://proxy.internal:8080', username: 'alice' });
    // 展示成功结果（绿色，含状态码与耗时）
    await waitFor(() => expect(screen.getByTestId('proxy-test-result')).toBeInTheDocument());
    expect(screen.getByTestId('proxy-test-result').textContent).toContain('200');
    expect(screen.getByTestId('proxy-test-result').textContent).toContain('123 ms');
  });

  it('FR-147：测试请求携带表单当前代理与新密码', async () => {
    vi.spyOn(api, 'getSettings').mockResolvedValue(启用样例);
    桩动态配置();
    const testProxy = vi
      .spyOn(api, 'testProxy')
      .mockResolvedValue({ ok: true, status: 200, elapsed_ms: 1 });
    renderPage();

    await waitFor(() => expect(screen.getByText('通用网络代理')).toBeInTheDocument());
    fireEvent.change(screen.getByLabelText('URL'), {
      target: { value: 'socks5://proxy.local:1080' },
    });
    fireEvent.change(screen.getByLabelText('用户名'), { target: { value: 'bob' } });
    fireEvent.change(screen.getByLabelText('密码'), { target: { value: 'new-secret' } });

    fireEvent.click(screen.getByTestId('proxy-test-open'));
    await waitFor(() => expect(screen.getByTestId('proxy-test-url-input')).toBeInTheDocument());
    fireEvent.click(screen.getByTestId('proxy-test-button'));

    await waitFor(() => expect(testProxy).toHaveBeenCalledTimes(1));
    const [, options] = testProxy.mock.calls[0];
    expect(options?.proxy).toEqual({
      url: 'socks5://proxy.local:1080',
      username: 'bob',
      password: 'new-secret',
    });
  });

  it('FR-147：清除密码后测试请求发送空 password', async () => {
    vi.spyOn(api, 'getSettings').mockResolvedValue(启用样例);
    桩动态配置();
    const testProxy = vi
      .spyOn(api, 'testProxy')
      .mockResolvedValue({ ok: true, status: 200, elapsed_ms: 1 });
    renderPage();

    await waitFor(() => expect(screen.getByText('通用网络代理')).toBeInTheDocument());
    fireEvent.click(screen.getByRole('button', { name: '清除密码' }));
    fireEvent.click(screen.getByTestId('proxy-test-open'));
    await waitFor(() => expect(screen.getByTestId('proxy-test-url-input')).toBeInTheDocument());
    fireEvent.click(screen.getByTestId('proxy-test-button'));

    await waitFor(() => expect(testProxy).toHaveBeenCalledTimes(1));
    const [, options] = testProxy.mock.calls[0];
    expect(options?.proxy).toEqual({
      url: 'http://proxy.internal:8080',
      username: 'alice',
      password: '',
    });
  });

  it('FR-147：代理 URL 为空时测试请求显式测直连', async () => {
    vi.spyOn(api, 'getSettings').mockResolvedValue(未启用样例);
    桩动态配置();
    const testProxy = vi
      .spyOn(api, 'testProxy')
      .mockResolvedValue({ ok: true, status: 200, elapsed_ms: 1 });
    renderPage();

    await waitFor(() => expect(screen.getByText('通用网络代理')).toBeInTheDocument());
    fireEvent.click(screen.getByTestId('proxy-test-open'));
    await waitFor(() => expect(screen.getByTestId('proxy-test-url-input')).toBeInTheDocument());
    fireEvent.click(screen.getByTestId('proxy-test-button'));

    await waitFor(() => expect(testProxy).toHaveBeenCalledTimes(1));
    const [, options] = testProxy.mock.calls[0];
    expect(options?.proxy).toEqual({ url: '' });
  });

  it('FR-147：中断测试后重开模态框不会卡在 loading', async () => {
    vi.spyOn(api, 'getSettings').mockResolvedValue(启用样例);
    桩动态配置();
    vi.spyOn(api, 'testProxy').mockImplementation(() => new Promise(() => {}));
    renderPage();

    await waitFor(() => expect(screen.getByText('通用网络代理')).toBeInTheDocument());
    fireEvent.click(screen.getByTestId('proxy-test-open'));
    await waitFor(() => expect(screen.getByTestId('proxy-test-url-input')).toBeInTheDocument());
    fireEvent.click(screen.getByTestId('proxy-test-button'));
    await waitFor(() => expect(screen.getByTestId('proxy-test-button')).toBeDisabled());

    fireEvent.click(screen.getByRole('button', { name: '关闭连通性测试' }));
    fireEvent.click(screen.getByTestId('proxy-test-open'));

    const button = await screen.findByTestId('proxy-test-button');
    expect(button).not.toBeDisabled();
    expect(button).toHaveTextContent('测试');
  });

  it('F-09：测试运行时编辑目标 URL 会中断旧请求、解除 loading 且忽略旧结果', async () => {
    vi.spyOn(api, 'getSettings').mockResolvedValue(启用样例);
    桩动态配置();
    const pending = 延迟结果<{ ok: boolean; status: number; elapsed_ms: number }>();
    let signal: AbortSignal | undefined;
    vi.spyOn(api, 'testProxy').mockImplementation((_url, options) => {
      signal = options?.signal;
      return pending.promise;
    });
    renderPage();

    await waitFor(() => expect(screen.getByText('通用网络代理')).toBeInTheDocument());
    fireEvent.click(screen.getByTestId('proxy-test-open'));
    const input = await screen.findByTestId('proxy-test-url-input');
    fireEvent.click(screen.getByTestId('proxy-test-button'));
    const button = screen.getByTestId('proxy-test-button');
    await waitFor(() => expect(button).toBeDisabled());

    fireEvent.change(input, { target: { value: 'https://new-target.example' } });

    expect.soft(signal?.aborted).toBe(true);
    expect.soft(button).not.toBeDisabled();
    await act(async () => {
      pending.resolve({ ok: true, status: 200, elapsed_ms: 9 });
      await pending.promise;
    });
    expect((input as HTMLInputElement).value).toBe('https://new-target.example');
    expect(screen.queryByTestId('proxy-test-result')).not.toBeInTheDocument();
  });

  it('F-09：测试运行时重置目标 URL 会中断旧请求、解除 loading 且忽略旧结果', async () => {
    vi.spyOn(api, 'getSettings').mockResolvedValue(启用样例);
    桩动态配置();
    const pending = 延迟结果<{ ok: boolean; status: number; elapsed_ms: number }>();
    let signal: AbortSignal | undefined;
    vi.spyOn(api, 'testProxy').mockImplementation((_url, options) => {
      signal = options?.signal;
      return pending.promise;
    });
    renderPage();

    await waitFor(() => expect(screen.getByText('通用网络代理')).toBeInTheDocument());
    fireEvent.click(screen.getByTestId('proxy-test-open'));
    const input = await screen.findByTestId('proxy-test-url-input');
    fireEvent.change(input, { target: { value: 'https://old-target.example' } });
    fireEvent.click(screen.getByTestId('proxy-test-button'));
    const button = screen.getByTestId('proxy-test-button');
    const resetButton = screen.getByTestId('proxy-test-reset-button');
    await waitFor(() => expect(button).toBeDisabled());

    expect.soft(resetButton).not.toBeDisabled();
    fireEvent.click(resetButton);

    expect.soft(signal?.aborted).toBe(true);
    expect.soft(button).not.toBeDisabled();
    expect.soft((input as HTMLInputElement).value).toBe('https://www.google.com');
    await act(async () => {
      pending.resolve({ ok: true, status: 200, elapsed_ms: 9 });
      await pending.promise;
    });
    expect(screen.queryByTestId('proxy-test-result')).not.toBeInTheDocument();
  });

  it('FR-128：模态框内点「测试」调 testProxy 并展示连通失败结果', async () => {
    vi.spyOn(api, 'getSettings').mockResolvedValue(启用样例);
    桩动态配置();
    vi.spyOn(api, 'testProxy').mockResolvedValue({
      ok: false,
      elapsed_ms: 500,
      error: '连接失败',
    });
    renderPage();

    await waitFor(() => expect(screen.getByText('通用网络代理')).toBeInTheDocument());
    fireEvent.click(screen.getByTestId('proxy-test-open'));
    await waitFor(() => expect(screen.getByTestId('proxy-test-url-input')).toBeInTheDocument());

    fireEvent.change(screen.getByTestId('proxy-test-url-input'), {
      target: { value: 'http://127.0.0.1:1' },
    });
    fireEvent.click(screen.getByTestId('proxy-test-button'));

    await waitFor(() => expect(screen.getByTestId('proxy-test-result')).toBeInTheDocument());
    expect(screen.getByTestId('proxy-test-result').textContent).toContain('连接失败');
  });

  it('FR-128：模态框内 URL 为空时不调 testProxy，展示提示', async () => {
    vi.spyOn(api, 'getSettings').mockResolvedValue(未启用样例);
    桩动态配置();
    const testProxy = vi.spyOn(api, 'testProxy');
    renderPage();

    await waitFor(() => expect(screen.getByText('通用网络代理')).toBeInTheDocument());
    // 打开模态框（预填占位值），随后手动清空 URL
    fireEvent.click(screen.getByTestId('proxy-test-open'));
    await waitFor(() => expect(screen.getByTestId('proxy-test-url-input')).toBeInTheDocument());

    // 清空 URL 后直接点测试
    fireEvent.change(screen.getByTestId('proxy-test-url-input'), { target: { value: '' } });
    fireEvent.click(screen.getByTestId('proxy-test-button'));

    // 不应调用 testProxy
    expect(testProxy).not.toHaveBeenCalled();
    // 应展示错误提示
    await waitFor(() => expect(screen.getByTestId('proxy-test-error')).toBeInTheDocument());
  });

  // ===== 防卡死 + 清除代理 + 错误展示完善 =====

  it('F-01：清除代理后重新填写三字段，保存与测试都发送新值', async () => {
    vi.spyOn(api, 'getSettings').mockResolvedValue(启用样例);
    桩动态配置();
    vi.spyOn(api, 'updateProtectionConfig').mockImplementation((c) => Promise.resolve(c));
    vi.spyOn(api, 'updateDynamicConfig').mockImplementation((c) => Promise.resolve(c));
    const update = vi.spyOn(api, 'updateSettings').mockResolvedValue(启用样例);
    const testProxy = vi
      .spyOn(api, 'testProxy')
      .mockResolvedValue({ ok: true, status: 200, elapsed_ms: 1 });
    renderPage();

    await waitFor(() => expect(screen.getByText('通用网络代理')).toBeInTheDocument());
    fireEvent.click(screen.getByTestId('proxy-clear'));
    fireEvent.change(screen.getByLabelText('URL'), {
      target: { value: 'socks5h://replacement.proxy:1080' },
    });
    fireEvent.change(screen.getByLabelText('用户名'), { target: { value: 'replacement-user' } });
    fireEvent.change(screen.getByLabelText('密码'), { target: { value: 'replacement-secret' } });

    fireEvent.click(screen.getByTestId('proxy-test-open'));
    await waitFor(() => expect(screen.getByTestId('proxy-test-url-input')).toBeInTheDocument());
    fireEvent.click(screen.getByTestId('proxy-test-button'));
    await waitFor(() => expect(testProxy).toHaveBeenCalledTimes(1));
    fireEvent.click(screen.getByRole('button', { name: '关闭连通性测试' }));

    fireEvent.click(screen.getByRole('button', { name: '保存' }));
    await waitFor(() => expect(update).toHaveBeenCalledTimes(1));

    const expectedProxy = {
      url: 'socks5h://replacement.proxy:1080',
      username: 'replacement-user',
      password: 'replacement-secret',
    };
    expect(testProxy.mock.calls[0][1]?.proxy).toEqual(expectedProxy);
    expect(update.mock.calls[0][0].network_proxy?.proxy).toEqual(expectedProxy);
  });

  it('代理已配置时「清除代理」按钮出现、点后 URL 输入清空、保存发空 URL', async () => {
    vi.spyOn(api, 'getSettings').mockResolvedValue(启用样例);
    桩动态配置();
    vi.spyOn(api, 'updateProtectionConfig').mockImplementation((c) => Promise.resolve(c));
    vi.spyOn(api, 'updateDynamicConfig').mockImplementation((c) => Promise.resolve(c));
    const update = vi.spyOn(api, 'updateSettings').mockResolvedValue(启用样例);
    renderPage();

    await waitFor(() => expect(screen.getByText('通用网络代理')).toBeInTheDocument());
    // 启用样例已配 URL：http://proxy.internal:8080 → 「清除代理」按钮应在场
    const clearBtn = screen.getByTestId('proxy-clear');
    expect(clearBtn).toBeInTheDocument();

    fireEvent.click(clearBtn);
    // URL 输入框被清空
    expect((screen.getByLabelText('URL') as HTMLInputElement).value).toBe('');

    // 保存：发空 URL → 后端 `rebuild_proxy_url` 归一为 None、清空代理槽
    fireEvent.click(screen.getByRole('button', { name: '保存' }));
    await waitFor(() => expect(update).toHaveBeenCalledTimes(1));
    const np = update.mock.calls[0][0].network_proxy!;
    expect(np.proxy.url).toBe('');
    expect(np.proxy.username).toBe('');
    // password 字段省略（清除代理场景不重置密码，原有密码仍保留在内存槽，不通过 PATCH 抹除）
    expect(np.proxy.password).toBeUndefined();
  });

  it('「清除代理」不影响 no_proxy', async () => {
    vi.spyOn(api, 'getSettings').mockResolvedValue(启用样例);
    桩动态配置();
    vi.spyOn(api, 'updateProtectionConfig').mockImplementation((c) => Promise.resolve(c));
    vi.spyOn(api, 'updateDynamicConfig').mockImplementation((c) => Promise.resolve(c));
    const update = vi.spyOn(api, 'updateSettings').mockResolvedValue(启用样例);
    renderPage();

    await waitFor(() => expect(screen.getByText('通用网络代理')).toBeInTheDocument());
    fireEvent.click(screen.getByTestId('proxy-clear'));
    fireEvent.click(screen.getByRole('button', { name: '保存' }));

    await waitFor(() => expect(update).toHaveBeenCalledTimes(1));
    const np = update.mock.calls[0][0].network_proxy!;
    // 代理被清空；no_proxy 仍照原样提交
    expect(np.proxy.url).toBe('');
    expect(np.no_proxy).toBe('localhost,127.0.0.1');
  });

  it('未配置代理时「清除代理」按钮不出现', async () => {
    vi.spyOn(api, 'getSettings').mockResolvedValue(未启用样例);
    桩动态配置();
    renderPage();

    await waitFor(() => expect(screen.getByText('通用网络代理')).toBeInTheDocument());
    expect(screen.queryByTestId('proxy-clear')).not.toBeInTheDocument();
  });

  it('mod 测试：testProxy 抛超时错误时展示「请求超时：…」友好提示', async () => {
    vi.spyOn(api, 'getSettings').mockResolvedValue(启用样例);
    桩动态配置();
    vi.spyOn(api, 'testProxy').mockRejectedValue(new api.ApiError(408, 'timeout', '请求超时'));
    renderPage();

    await waitFor(() => expect(screen.getByText('通用网络代理')).toBeInTheDocument());
    fireEvent.click(screen.getByTestId('proxy-test-open'));
    await waitFor(() => expect(screen.getByTestId('proxy-test-url-input')).toBeInTheDocument());

    fireEvent.change(screen.getByTestId('proxy-test-url-input'), {
      target: { value: 'https://example.com' },
    });
    fireEvent.click(screen.getByTestId('proxy-test-button'));

    const err = await screen.findByTestId('proxy-test-error');
    // 错误信息带上「请求超时」前缀
    expect(err.textContent).toContain('请求超时');
  });

  it('mod 测试：测试结果成功时耗时格式化为 `123 ms` / `1.23 s`', async () => {
    vi.spyOn(api, 'getSettings').mockResolvedValue(启用样例);
    桩动态配置();
    vi.spyOn(api, 'testProxy').mockResolvedValue({ ok: true, status: 200, elapsed_ms: 123 });
    renderPage();

    await waitFor(() => expect(screen.getByText('通用网络代理')).toBeInTheDocument());
    fireEvent.click(screen.getByTestId('proxy-test-open'));
    await waitFor(() => expect(screen.getByTestId('proxy-test-url-input')).toBeInTheDocument());

    fireEvent.click(screen.getByTestId('proxy-test-button'));
    const result = await screen.findByTestId('proxy-test-result');
    expect(result.textContent).toContain('123 ms');
  });

  it('mod 测试：URL 改动清空旧结果、点重置按钮把 URL 恢复成占位值', async () => {
    vi.spyOn(api, 'getSettings').mockResolvedValue(启用样例);
    桩动态配置();
    renderPage();

    await waitFor(() => expect(screen.getByText('通用网络代理')).toBeInTheDocument());
    fireEvent.click(screen.getByTestId('proxy-test-open'));
    const input = await screen.findByTestId('proxy-test-url-input');
    // 打开即预填占位值（FR-146：不预填代理地址）
    expect((input as HTMLInputElement).value).toBe('https://www.google.com');

    // 改成自定义 URL
    fireEvent.change(input, { target: { value: 'https://changed.example' } });
    expect((input as HTMLInputElement).value).toBe('https://changed.example');

    // 点「重置」按钮：恢复成占位值（placeholder 不是 value）
    fireEvent.click(screen.getByTestId('proxy-test-reset-button'));
    const resetInput = screen.getByTestId('proxy-test-url-input') as HTMLInputElement;
    expect(resetInput.value).toBe('https://www.google.com');
  });
});
