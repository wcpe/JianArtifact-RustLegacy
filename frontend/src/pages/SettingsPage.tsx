// 设置页（FR-87 只读 + FR-88 可编辑热替换 + FR-106 动态配置 + FR-129 顶部 Tab 分页，仅管理员）：
// 顶部水平 Tab 分页（网络代理 / 限制与配额 / 可观测性 / 漏洞库 / 安全·会话 / 防护配置），
// 每节一个 Tab.Panel、切换不滚动——彻底消除原锚点长滚动 scroll-spy 的高亮 / hover 错位（FR-129）。
// 底部**只有一个** sticky 全局保存按钮：一次性提交 PATCH /settings（即时生效）+
// PATCH /settings/dynamic（重启生效）+ PATCH /protection/config（即时生效，FR-129 起并入单一保存）。
//
// 在线更新已迁至「系统」页（FR-109，SystemPage），本页不再含应用更新卡片。
//
// 数据来自后端 GET /api/v1/settings（已脱敏：代理 URL 去凭据）、GET /api/v1/settings/dynamic（非密钥项）
// 与 GET /api/v1/protection/config（防护各维度）。保存走对应 PATCH（代理凭据只入内存槽、不写回 TOML / 不回显）。

import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  Alert,
  Box,
  Stack,
  Title,
  Text,
  Card,
  Group,
  Badge,
  Button,
  Loader,
  Center,
  TextInput,
  Switch,
  PasswordInput,
  NumberInput,
  Tabs,
  Modal,
} from '@mantine/core';
import { IconDeviceFloppy } from '@tabler/icons-react';
import * as api from '../api/endpoints';
import type {
  SettingsView,
  ProxyEntryPatch,
  DynamicConfig,
  ProxyTestResult,
  ProtectionConfig,
} from '../api/types';
import { errorMessage, linesToList } from '../lib/format';
import { ErrorAlert } from '../components/ErrorAlert';
import { density } from '../theme/density';
import { ProtectionConfigSection } from './ProtectionConfigSection';

/** 单代理三字段（URL / 用户名 / 密码）一组（FR-100）。密码框始终空、不回显；已配置时标徽标 + 提供清除密码。 */
interface ProxyFieldsProps {
  title: string;
  urlPlaceholder: string;
  url: string;
  onUrlChange: (v: string) => void;
  username: string;
  onUsernameChange: (v: string) => void;
  password: string;
  onPasswordChange: (v: string) => void;
  hasPassword: boolean;
  passwordCleared: boolean;
  onClearPassword: () => void;
}

function ProxyFields(props: ProxyFieldsProps) {
  const {
    title,
    urlPlaceholder,
    url,
    onUrlChange,
    username,
    onUsernameChange,
    password,
    onPasswordChange,
    hasPassword,
    passwordCleared,
    onClearPassword,
  } = props;
  const { t } = useTranslation('settings');
  return (
    <Stack gap="xs">
      <Group gap="xs">
        <Text size="sm" fw={600}>
          {title}
        </Text>
        {/* 已配置密码标识（绝不回显密码本体）；点过清除则提示本次保存将清空 */}
        {hasPassword && !passwordCleared && (
          <Badge size="sm" color="blue" variant="light">
            {t('proxyFields.passwordConfigured')}
          </Badge>
        )}
        {passwordCleared && (
          <Badge size="sm" color="orange" variant="light">
            {t('proxyFields.passwordWillClear')}
          </Badge>
        )}
      </Group>
      <TextInput
        label={t('proxyFields.urlLabel')}
        placeholder={urlPlaceholder}
        value={url}
        onChange={(e) => onUrlChange(e.currentTarget.value)}
      />
      <TextInput
        label={t('proxyFields.usernameLabel')}
        placeholder={t('proxyFields.usernamePlaceholder')}
        value={username}
        onChange={(e) => onUsernameChange(e.currentTarget.value)}
      />
      <PasswordInput
        label={t('proxyFields.passwordLabel')}
        placeholder={t('proxyFields.passwordPlaceholder')}
        value={password}
        onChange={(e) => onPasswordChange(e.currentTarget.value)}
      />
      {/* 仅在已配置密码时提供「清除密码」动作（发 password: ""）；填了新密码或已点清除则不再显示 */}
      {hasPassword && !passwordCleared && !password && (
        <Group>
          <Button size="xs" variant="subtle" color="red" onClick={onClearPassword}>
            {t('proxyFields.clearPassword')}
          </Button>
        </Group>
      )}
    </Stack>
  );
}

/** 重启生效徽标（动态配置各节共用：保存后须重启才生效，区别于代理 / 防护即时生效）。 */
function RestartHintBadge() {
  const { t } = useTranslation('settings');
  return (
    <Badge size="sm" color="yellow" variant="light">
      {t('restartHint')}
    </Badge>
  );
}

/** 设置页。 */
export function SettingsPage() {
  const { t } = useTranslation('settings');
  const [settings, setSettings] = useState<SettingsView | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  // 当前激活的顶部 Tab（FR-129）：仅驱动分页显示，不与滚动联动（根因消除锚点高亮 / hover 错位）。
  const [activeTab, setActiveTab] = useState<string>('proxy');

  // —— 可编辑表单态（FR-88 + FR-100 + FR-146 单槽收敛）——
  // 单一通用代理拆三字段：URL（脱敏 host，无凭据）、用户名（回显）、密码（不回显）。
  // 密码框始终空：留空保存=省略 password 字段（保留现有）；填值=设置；另有「清除密码」动作发空串清空。
  const [proxyUrl, setProxyUrl] = useState('');
  const [proxyUser, setProxyUser] = useState('');
  const [proxyPass, setProxyPass] = useState('');
  const [proxyHasPass, setProxyHasPass] = useState(false);
  // 「清除密码」动作的标记：点了清除即在本次 PATCH 发 password: "" 清空代理密码。
  const [proxyClearPass, setProxyClearPass] = useState(false);
  // 「清除代理」动作的标记（清空槽位的 URL / 用户名 / 密码并清除 has_password），
  // 与「清除密码」互斥：点过之后在保存时构造 PATCH 载荷跳过另一项。
  const [proxyClearProxy, setProxyClearProxy] = useState(false);
  const [noProxy, setNoProxy] = useState('');
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);

  // —— 代理连通性测试（FR-128 回流 UX：单槽「测试」按钮 → 模态框）——
  const [testModalOpen, setTestModalOpen] = useState(false);
  const [testUrl, setTestUrl] = useState('');
  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState<ProxyTestResult | null>(null);
  const [testError, setTestError] = useState<string | null>(null);
  // 测试请求的 abort 控制器：用于模态框关闭或 URL 改动时打断旧 promise 防 setState-after-unmount
  // 与「卡死时仍写状态」的竞态。
  const testAbortRef = useRef<AbortController | null>(null);
  // 测试是否超时（用于友好提示「请求超时」）：超时由 `request` 内部 AbortController 触发后归一抛出。
  const [testTimedOut, setTestTimedOut] = useState(false);

  // —— 系统配置（动态配置面板，FR-106）——
  // limits / observability / vuln / auth 非密钥项；保存落库、**重启生效**（无热替换槽）。
  // 并入对应 Tab、随**全局保存**一并 PATCH（不再有独立保存按钮）。
  const [dynamic, setDynamic] = useState<DynamicConfig | null>(null);
  const [dynamicError, setDynamicError] = useState<string | null>(null);

  // —— 防护配置（FR-110 + FR-129：state 上提、并入单一保存）——
  // GET /protection/config 加载；IP 名单以文本域为准（保存前归并）；随全局保存一并 PATCH（即时生效）。
  const [protection, setProtection] = useState<ProtectionConfig | null>(null);
  const [protectionAllowText, setProtectionAllowText] = useState('');
  const [protectionDenyText, setProtectionDenyText] = useState('');
  const [protectionLoading, setProtectionLoading] = useState(true);
  const [protectionError, setProtectionError] = useState<string | null>(null);

  // 用一份设置填充表单态。
  function fillForm(s: SettingsView) {
    setSettings(s);
    // 通用代理单槽（FR-146）：URL / 用户名回显预填，密码框始终空、仅以 has_password 标识是否已配置（FR-100）。
    setProxyUrl(s.network_proxy.proxy.url ?? '');
    setProxyUser(s.network_proxy.proxy.username ?? '');
    setProxyPass('');
    setProxyHasPass(s.network_proxy.proxy.has_password);
    setProxyClearPass(false);
    setProxyClearProxy(false);
    setNoProxy(s.network_proxy.no_proxy ?? '');
  }

  // 用一份防护配置填充表单态（含 IP 名单文本域）。
  function fillProtection(cfg: ProtectionConfig) {
    setProtection(cfg);
    setProtectionAllowText(cfg.ip_list.allow.join('\n'));
    setProtectionDenyText(cfg.ip_list.deny.join('\n'));
  }

  useEffect(() => {
    api
      .getSettings()
      .then(fillForm)
      .catch((err) => setError(errorMessage(err)))
      .finally(() => setLoading(false));
  }, []);

  // 动态配置独立加载：失败不阻塞代理 / 主表单，仅在对应节内提示；加载失败时全局保存跳过 dynamic PATCH。
  useEffect(() => {
    api
      .getDynamicConfig()
      .then(setDynamic)
      .catch((err) => setDynamicError(errorMessage(err)));
  }, []);

  // 防护配置独立加载（FR-110/129）：失败仅在防护 Tab 内提示；加载失败时全局保存跳过 protection PATCH。
  useEffect(() => {
    api
      .getProtectionConfig()
      .then(fillProtection)
      .catch((err) => setProtectionError(errorMessage(err)))
      .finally(() => setProtectionLoading(false));
  }, []);

  // 不可变更新动态配置某节的某字段（保持薄、复用于所有数值 / 开关项）。
  function patchDynamic<K extends keyof DynamicConfig>(section: K, value: DynamicConfig[K]): void {
    setDynamic((prev) => (prev ? { ...prev, [section]: value } : prev));
    setSaved(false);
  }

  // 不可变更新防护配置某维度的某字段（保持其余不变，整体可回传）。
  function patchProtection<K extends keyof ProtectionConfig>(
    key: K,
    value: ProtectionConfig[K],
  ): void {
    setProtection((prev) => (prev ? { ...prev, [key]: value } : prev));
    setSaved(false);
  }

  if (loading) {
    return (
      <Center h={200}>
        <Loader />
      </Center>
    );
  }

  if (!settings) {
    return (
      <Stack>
        <Title order={2}>{t('pageTitle')}</Title>
        {error && <ErrorAlert message={error} />}
      </Stack>
    );
  }

  // 组装通用代理 PATCH 项（FR-100 + FR-146 单槽）：url / username 照填；
  // 密码三态——填了新密码 → 带 password（设置）；点了「清除密码」→ 带 password: ""（清空）；
  // 否则省略 password 字段（保留现有）。
  // 「清除代理」语义：保留 entry 但 url 置为空字符串——后端 `rebuild_proxy_url` 会把它归一为 None、清空代理槽；
  // 此时 password 字段省略，避免与空 password 同时出现歧义（「清除代理」覆盖「清除密码」）。
  function buildProxyPatchForSave(): ProxyEntryPatch {
    if (proxyClearProxy) {
      return { url: '', username: '' };
    }
    const entry: ProxyEntryPatch = { url: proxyUrl.trim(), username: proxyUser.trim() };
    if (proxyPass) {
      entry.password = proxyPass;
    } else if (proxyClearPass) {
      entry.password = '';
    }
    return entry;
  }

  // FR-147：测试走表单当前值，不要求先保存；URL 空或点过清除代理即显式测直连。
  function buildProxyPatchForTest(): ProxyEntryPatch {
    if (proxyClearProxy || proxyUrl.trim() === '') {
      return { url: '' };
    }
    const entry: ProxyEntryPatch = { url: proxyUrl.trim(), username: proxyUser.trim() };
    if (proxyPass !== '') {
      entry.password = proxyPass;
    } else if (proxyClearPass) {
      entry.password = '';
    }
    return entry;
  }

  // 把耗时毫秒格式化为可读字符串：>=1s 显示秒、否则毫秒；带小数位便于判断瞬时响应与慢代理差异。
  function 格式化耗时(ms: number): string {
    if (ms >= 1000) {
      return `${(ms / 1000).toFixed(2)} s`;
    }
    return `${ms} ms`;
  }

  // 把字符串复制到剪贴板：优先用 navigator.clipboard.writeText（异步、需 https 或 localhost）；
  // 失败时回退到 execCommand('copy') 选中文本复制。
  // 错误展示的「复制错误」按钮、测试失败原因复制都走它。
  async function copyText(text: string | null): Promise<void> {
    if (!text) return;
    try {
      if (navigator.clipboard && typeof navigator.clipboard.writeText === 'function') {
        await navigator.clipboard.writeText(text);
        return;
      }
    } catch {
      // 忽略：继续走 fallback
    }
    try {
      const ta = document.createElement('textarea');
      ta.value = text;
      ta.style.position = 'fixed';
      ta.style.opacity = '0';
      document.body.appendChild(ta);
      ta.focus();
      ta.select();
      document.execCommand('copy');
      document.body.removeChild(ta);
    } catch {
      // 静默失败：复制失败不影响主流程
    }
  }

  // 取消当前测试并立即解除 loading；清空 controller 身份后，旧 promise 完成时无法回写状态。
  function cancelCurrentProxyTest() {
    const controller = testAbortRef.current;
    testAbortRef.current = null;
    controller?.abort();
    setTesting(false);
  }

  // 关闭测试模态框：取消挂起请求，避免离开后旧 promise 写状态。
  function closeTestModal() {
    cancelCurrentProxyTest();
    setTestModalOpen(false);
    setTestTimedOut(false);
  }

  // 打开代理连通性测试模态框（FR-128 回流 UX + FR-146/147 缺陷修复）：
  // 预填**目标 URL 占位值**（testUrlPlaceholder），不再预填代理地址——测试输入的是目标站点、不是代理本身。
  function openTestModal() {
    // 重新打开前取消挂起请求，保证只会有一个测试在跑，并复位 loading 防止按钮卡死。
    cancelCurrentProxyTest();
    setTestUrl(t('proxy.testUrlPlaceholder'));
    setTestResult(null);
    setTestError(null);
    setTestTimedOut(false);
    setTestModalOpen(true);
  }

  // 模态框内执行连通性测试：发 POST /settings/proxy-test，展示状态码 / 耗时 / 失败原因。
  // 关键防卡死：① 每次新建 AbortController 取消上一次；② 后端 / 客户端 10s 双层超时
  // （见 `api.testProxy` 与 `request`），超时归一为 `ApiError(408, 'timeout')`、展示「请求超时」友好提示；
  // ③ 模态框关闭或 URL 改动时主动 abort，杜绝 setState-after-unmount 与 UI 卡死竞态。
  async function handleProxyTest() {
    const url = testUrl.trim();
    if (!url) {
      setTestError(t('proxy.testUrlRequired'));
      return;
    }
    // 取消挂起的旧测试，保证同一时刻仅一个测试
    testAbortRef.current?.abort();
    const controller = new AbortController();
    testAbortRef.current = controller;
    setTesting(true);
    setTestResult(null);
    setTestError(null);
    setTestTimedOut(false);
    try {
      const result = await api.testProxy(url, {
        proxy: buildProxyPatchForTest(),
        signal: controller.signal,
      });
      if (testAbortRef.current !== controller || controller.signal.aborted) return;
      setTestResult(result);
    } catch (err) {
      if (testAbortRef.current !== controller || controller.signal.aborted) return;
      const msg = errorMessage(err);
      // ApiError(408, 'timeout') 由 client.request 抛；'aborted'(0) 由外部 signal 触发；二者不应混入 UI 提示。
      const isTimeout = err instanceof api.ApiError && err.status === 408;
      setTestTimedOut(isTimeout);
      setTestError(msg);
    } finally {
      if (testAbortRef.current === controller) {
        testAbortRef.current = null;
        setTesting(false);
      }
    }
  }

  // 全局保存（FR-103 + FR-129）：一次性提交三处写入——
  // ① PATCH /settings（网络代理，即时生效，部分 PATCH 只发 network_proxy）；
  // ② 若动态配置已加载则 PATCH /settings/dynamic（limits/observability/vuln/auth，重启生效）；
  // ③ 若防护配置已加载则 PATCH /protection/config（即时生效，IP 名单以文本域为准、保存前归并）。
  // 顺序提交，任一失败聚合到 saveError、成功显「已保存」。
  // 「清除代理」语义上覆盖「清除密码」：清代理会把 password 字段省略（URL 空字符串已表意清除）。
  async function handleSaveAll() {
    setSaving(true);
    setSaveError(null);
    setSaved(false);
    try {
      const updated = await api.updateSettings({
        network_proxy: {
          proxy: buildProxyPatchForSave(),
          no_proxy: noProxy.trim(),
        },
      });
      fillForm(updated);
      // 动态配置已加载才提交（未加载则跳过，不报错）
      if (dynamic) {
        const updatedDynamic = await api.updateDynamicConfig(dynamic);
        setDynamic(updatedDynamic);
      }
      // 防护配置已加载才提交（未加载则跳过，不报错）：IP 名单以文本域为准，提交前归并回配置。
      if (protection) {
        const updatedProtection = await api.updateProtectionConfig({
          ...protection,
          ip_list: {
            allow: linesToList(protectionAllowText),
            deny: linesToList(protectionDenyText),
          },
        });
        fillProtection(updatedProtection);
      }
      setSaved(true);
    } catch (err) {
      setSaveError(errorMessage(err));
    } finally {
      setSaving(false);
    }
  }

  return (
    <Stack gap={density.gridSpacing}>
      <Title order={2}>{t('pageTitle')}</Title>
      {error && <ErrorAlert message={error} />}

      {/* 代理连通性测试模态框（FR-128 回流 UX：单槽「测试」按钮弹出） */}
      <Modal
        opened={testModalOpen}
        onClose={closeTestModal}
        title={t('proxy.testModalTitle')}
        size="md"
        closeButtonProps={{ 'aria-label': t('proxy.closeTestModal') }}
      >
        <Stack gap="sm">
          <Text size="sm" c="dimmed">
            {t('proxy.testModalDesc')}
          </Text>
          <Group gap="xs" align="flex-end">
            <TextInput
              label={t('proxy.testUrlLabel')}
              placeholder={t('proxy.testUrlPlaceholder')}
              value={testUrl}
              onChange={(e) => {
                cancelCurrentProxyTest();
                setTestUrl(e.currentTarget.value);
                // URL 一变即取消旧请求并作废旧结果，避免旧 promise 回写本次测试状态。
                setTestResult(null);
                setTestError(null);
                setTestTimedOut(false);
              }}
              style={{ flex: 1 }}
              data-testid="proxy-test-url-input"
            />
            <Button
              size="xs"
              variant="subtle"
              // 「重置」按钮：把 URL 输入恢复成占位提示值（与打开模态框时的预填一致）。
              onClick={() => {
                cancelCurrentProxyTest();
                setTestUrl(t('proxy.testUrlPlaceholder'));
                setTestResult(null);
                setTestError(null);
                setTestTimedOut(false);
              }}
              data-testid="proxy-test-reset-button"
            >
              {t('proxy.resetUrl')}
            </Button>
            <Button
              loading={testing}
              disabled={testing}
              onClick={handleProxyTest}
              data-testid="proxy-test-button"
            >
              {testing ? t('proxy.testTesting') : t('proxy.testRunButton')}
            </Button>
          </Group>
          {/* 测试结果：成功绿色（含状态码 + 格式化耗时）、失败红色（含失败原因 + 耗时）*/}
          {testResult && (
            <Alert
              color={testResult.ok ? 'green' : 'red'}
              variant="light"
              data-testid="proxy-test-result"
            >
              {testResult.ok
                ? t('proxy.testResultOk', {
                    status: testResult.status,
                    // 耗时 >=1s 切秒，否则毫秒；便于直观判断代理健康与目标网络质量。
                    elapsed: 格式化耗时(testResult.elapsed_ms),
                  })
                : t('proxy.testResultFail', {
                    error: testResult.error ?? t('proxy.testResultFailNoError'),
                    elapsed: 格式化耗时(testResult.elapsed_ms),
                  })}
            </Alert>
          )}
          {/* 错误区：超时与「复制错误」按钮（仅存在 testError 时出现） */}
          {testError && (
            <Alert
              color={testTimedOut ? 'orange' : 'red'}
              variant="light"
              data-testid="proxy-test-error"
              icon={testTimedOut ? undefined : undefined}
            >
              <Group justify="space-between" align="center" wrap="nowrap">
                <Text size="sm" style={{ flex: 1 }}>
                  {testTimedOut ? t('proxy.timeout') + '：' + testError : testError}
                </Text>
                <Button
                  size="xs"
                  variant="subtle"
                  // 「复制错误」：避免手抄；jsdom 不一定支持 clipboard，回退用选中全文（fallback 在测试里 stub）。
                  onClick={() => copyText(testError)}
                  data-testid="proxy-test-copy-error-button"
                >
                  {t('proxy.copyError')}
                </Button>
              </Group>
            </Alert>
          )}
        </Stack>
      </Modal>

      {/* FR-129：顶部水平 Tab 分页。每节一个 Tab.Panel、切换不滚动；
          未激活面板不渲染（Mantine 默认），各节内容互不串味，根因消除锚点高亮 / hover 错位。 */}
      <Tabs value={activeTab} onChange={(v) => v && setActiveTab(v)}>
        <Tabs.List aria-label={t('navAriaLabel')}>
          <Tabs.Tab value="proxy">{t('proxy.title')}</Tabs.Tab>
          <Tabs.Tab value="limits">{t('limits.title')}</Tabs.Tab>
          <Tabs.Tab value="observability">{t('observability.title')}</Tabs.Tab>
          <Tabs.Tab value="vuln">{t('vuln.title')}</Tabs.Tab>
          <Tabs.Tab value="auth">{t('auth.title')}</Tabs.Tab>
          <Tabs.Tab value="protection">{t('nav.protection')}</Tabs.Tab>
        </Tabs.List>

        {/* —— 网络代理 Tab —— */}
        <Tabs.Panel value="proxy" pt="md">
          <Card component="section" withBorder padding={density.cardPadding} radius="md">
            <Title order={4}>{t('proxy.title')}</Title>
            <Text size="sm" c="dimmed" mb="sm">
              {t('proxy.desc')}
            </Text>
            <Stack gap="md">
              {/* 单一通用代理槽（FR-146）：URL 自填 scheme，经 Proxy::all 对全部出站请求生效 */}
              <Stack gap="xs">
                <ProxyFields
                  title={t('proxy.generalTitle')}
                  urlPlaceholder="socks5://host:1080 或 http://host:3128"
                  url={proxyUrl}
                  onUrlChange={(value) => {
                    setProxyUrl(value);
                    setProxyClearProxy(false);
                  }}
                  username={proxyUser}
                  onUsernameChange={(value) => {
                    setProxyUser(value);
                    setProxyClearProxy(false);
                  }}
                  password={proxyPass}
                  onPasswordChange={(value) => {
                    setProxyPass(value);
                    setProxyClearProxy(false);
                    if (value !== '') setProxyClearPass(false);
                  }}
                  hasPassword={proxyHasPass}
                  passwordCleared={proxyClearPass}
                  onClearPassword={() => setProxyClearPass(true)}
                />
                {/* 支持的 scheme 说明（socks5h 由代理端解析域名，受污染网络推荐） */}
                <Text size="xs" c="dimmed">
                  {t('proxy.schemeHint')}
                </Text>
                <Group gap="xs">
                  <Button
                    size="xs"
                    variant="light"
                    onClick={openTestModal}
                    data-testid="proxy-test-open"
                  >
                    {t('proxy.testButton')}
                  </Button>
                  {/* 「清除代理」按钮：仅当槽位已配置（URL 或 用户名非空且未点过清除）时出现。
                      点了之后清空 URL / 用户名 / 密码，no_proxy 不动。 */}
                  {(proxyUrl.trim() !== '' || proxyUser.trim() !== '') && !proxyClearProxy && (
                    <Button
                      size="xs"
                      variant="subtle"
                      color="red"
                      data-testid="proxy-clear"
                      onClick={() => {
                        setProxyUrl('');
                        setProxyUser('');
                        setProxyPass('');
                        setProxyHasPass(false);
                        // 标记清除代理；与清除密码互斥（保存时优先走清除代理分支）
                        setProxyClearProxy(true);
                        setProxyClearPass(false);
                      }}
                    >
                      {t('proxy.clearProxy')}
                    </Button>
                  )}
                  {proxyClearProxy && (
                    <Badge size="sm" color="orange" variant="light">
                      {t('proxy.proxyWillClear')}
                    </Badge>
                  )}
                </Group>
              </Stack>

              <TextInput
                label={t('proxy.noProxyLabel')}
                placeholder="localhost,127.0.0.1,.internal"
                value={noProxy}
                onChange={(e) => setNoProxy(e.currentTarget.value)}
              />
            </Stack>
          </Card>
        </Tabs.Panel>

        {/* —— 限制与配额 Tab（动态配置，FR-106：重启生效）—— */}
        <Tabs.Panel value="limits" pt="md">
          {dynamicError && <ErrorAlert message={dynamicError} />}
          <Card component="section" withBorder padding={density.cardPadding} radius="md">
            <Group gap="xs" mb="xs">
              <Title order={4}>{t('limits.title')}</Title>
              <RestartHintBadge />
            </Group>
            {!dynamic ? (
              <Center h={80}>
                <Loader size="sm" />
              </Center>
            ) : (
              <NumberInput
                label={t('limits.maxArtifactSizeLabel')}
                description={t('limits.maxArtifactSizeDesc')}
                placeholder={t('limits.maxArtifactSizePlaceholder')}
                min={0}
                value={dynamic.limits.max_artifact_size ?? ''}
                onChange={(v) =>
                  patchDynamic('limits', {
                    max_artifact_size: v === '' || v === null ? null : Number(v),
                  })
                }
              />
            )}
          </Card>
        </Tabs.Panel>

        {/* —— 可观测性 Tab —— */}
        <Tabs.Panel value="observability" pt="md">
          {dynamicError && <ErrorAlert message={dynamicError} />}
          <Card component="section" withBorder padding={density.cardPadding} radius="md">
            <Group gap="xs" mb="xs">
              <Title order={4}>{t('observability.title')}</Title>
              <RestartHintBadge />
            </Group>
            {!dynamic ? (
              <Center h={120}>
                <Loader size="sm" />
              </Center>
            ) : (
              <Stack gap="sm">
                <Group grow>
                  <NumberInput
                    label={t('observability.auditRetentionDays')}
                    min={0}
                    value={dynamic.audit.retention_days}
                    onChange={(v) =>
                      patchDynamic('audit', { ...dynamic.audit, retention_days: Number(v) || 0 })
                    }
                  />
                  <NumberInput
                    label={t('observability.auditMaxRows')}
                    min={0}
                    value={dynamic.audit.max_rows}
                    onChange={(v) =>
                      patchDynamic('audit', { ...dynamic.audit, max_rows: Number(v) || 0 })
                    }
                  />
                </Group>
                <Switch
                  label={t('observability.usageDetailEnabled')}
                  checked={dynamic.usage.detail_enabled}
                  onChange={(e) =>
                    patchDynamic('usage', {
                      ...dynamic.usage,
                      detail_enabled: e.currentTarget.checked,
                    })
                  }
                />
                <NumberInput
                  label={t('observability.usageMaxDetailRows')}
                  min={0}
                  value={dynamic.usage.max_detail_rows}
                  onChange={(v) =>
                    patchDynamic('usage', { ...dynamic.usage, max_detail_rows: Number(v) || 0 })
                  }
                />
                <Switch
                  label={t('observability.metricsEnabled')}
                  checked={dynamic.metrics.enabled}
                  onChange={(e) =>
                    patchDynamic('metrics', {
                      ...dynamic.metrics,
                      enabled: e.currentTarget.checked,
                    })
                  }
                />
                <Switch
                  label={t('observability.metricsAllowAnonymous')}
                  checked={dynamic.metrics.allow_anonymous}
                  onChange={(e) =>
                    patchDynamic('metrics', {
                      ...dynamic.metrics,
                      allow_anonymous: e.currentTarget.checked,
                    })
                  }
                />
                <Switch
                  label={t('observability.timeseriesEnabled')}
                  checked={dynamic.metrics_timeseries.enabled}
                  onChange={(e) =>
                    patchDynamic('metrics_timeseries', {
                      ...dynamic.metrics_timeseries,
                      enabled: e.currentTarget.checked,
                    })
                  }
                />
                <Group grow>
                  <NumberInput
                    label={t('observability.timeseriesSampleInterval')}
                    min={1}
                    value={dynamic.metrics_timeseries.sample_interval_secs}
                    onChange={(v) =>
                      patchDynamic('metrics_timeseries', {
                        ...dynamic.metrics_timeseries,
                        sample_interval_secs: Number(v) || 0,
                      })
                    }
                  />
                  <NumberInput
                    label={t('observability.timeseriesRetentionDays')}
                    min={0}
                    value={dynamic.metrics_timeseries.retention_days}
                    onChange={(v) =>
                      patchDynamic('metrics_timeseries', {
                        ...dynamic.metrics_timeseries,
                        retention_days: Number(v) || 0,
                      })
                    }
                  />
                </Group>
              </Stack>
            )}
          </Card>
        </Tabs.Panel>

        {/* —— 漏洞库 Tab —— */}
        <Tabs.Panel value="vuln" pt="md">
          {dynamicError && <ErrorAlert message={dynamicError} />}
          <Card component="section" withBorder padding={density.cardPadding} radius="md">
            <Group gap="xs" mb="xs">
              <Title order={4}>{t('vuln.title')}</Title>
              <RestartHintBadge />
            </Group>
            {!dynamic ? (
              <Center h={120}>
                <Loader size="sm" />
              </Center>
            ) : (
              <Stack gap="sm">
                <Switch
                  label={t('vuln.enabled')}
                  checked={dynamic.vuln.enabled}
                  onChange={(e) =>
                    patchDynamic('vuln', { ...dynamic.vuln, enabled: e.currentTarget.checked })
                  }
                />
                <TextInput
                  label={t('vuln.sourceBaseUrl')}
                  placeholder="https://osv-vulnerabilities.storage.googleapis.com"
                  value={dynamic.vuln.source_base_url}
                  onChange={(e) =>
                    patchDynamic('vuln', {
                      ...dynamic.vuln,
                      source_base_url: e.currentTarget.value,
                    })
                  }
                />
                <Group grow>
                  <NumberInput
                    label={t('vuln.refreshInterval')}
                    min={1}
                    value={dynamic.vuln.refresh_interval_secs}
                    onChange={(v) =>
                      patchDynamic('vuln', {
                        ...dynamic.vuln,
                        refresh_interval_secs: Number(v) || 0,
                      })
                    }
                  />
                  <NumberInput
                    label={t('vuln.downloadTimeout')}
                    min={1}
                    value={dynamic.vuln.download_timeout_secs}
                    onChange={(v) =>
                      patchDynamic('vuln', {
                        ...dynamic.vuln,
                        download_timeout_secs: Number(v) || 0,
                      })
                    }
                  />
                </Group>
              </Stack>
            )}
          </Card>
        </Tabs.Panel>

        {/* —— 安全 / 会话 Tab —— */}
        <Tabs.Panel value="auth" pt="md">
          {dynamicError && <ErrorAlert message={dynamicError} />}
          <Card component="section" withBorder padding={density.cardPadding} radius="md">
            <Group gap="xs" mb="xs">
              <Title order={4}>{t('auth.title')}</Title>
              <RestartHintBadge />
            </Group>
            <Text size="xs" c="dimmed" mb="xs">
              {t('auth.desc')}
            </Text>
            {!dynamic ? (
              <Center h={80}>
                <Loader size="sm" />
              </Center>
            ) : (
              <Group grow>
                <NumberInput
                  label={t('auth.sessionTtl')}
                  min={1}
                  value={dynamic.auth.session_ttl_secs}
                  onChange={(v) =>
                    patchDynamic('auth', { ...dynamic.auth, session_ttl_secs: Number(v) || 0 })
                  }
                />
                <NumberInput
                  label={t('auth.loginMaxFailures')}
                  min={0}
                  value={dynamic.auth.login_max_failures}
                  onChange={(v) =>
                    patchDynamic('auth', { ...dynamic.auth, login_max_failures: Number(v) || 0 })
                  }
                />
                <NumberInput
                  label={t('auth.loginLockoutSecs')}
                  min={1}
                  value={dynamic.auth.login_lockout_secs}
                  onChange={(v) =>
                    patchDynamic('auth', { ...dynamic.auth, login_lockout_secs: Number(v) || 0 })
                  }
                />
              </Group>
            )}
          </Card>
        </Tabs.Panel>

        {/* —— 防护配置 Tab（FR-110 并入设置页 + FR-129 并入单一保存）——
            state 由设置页托管，随全局保存一并 PATCH /protection/config（即时生效），无独立保存按钮。 */}
        <Tabs.Panel value="protection" pt="md">
          <ProtectionConfigSection
            config={protection}
            allowText={protectionAllowText}
            denyText={protectionDenyText}
            loading={protectionLoading}
            error={protectionError}
            onAllowTextChange={(v) => {
              setProtectionAllowText(v);
              setSaved(false);
            }}
            onDenyTextChange={(v) => {
              setProtectionDenyText(v);
              setSaved(false);
            }}
            onPatch={patchProtection}
          />
        </Tabs.Panel>
      </Tabs>

      {/* —— 单个全局保存（FR-103 + FR-129）——
          固定为 sticky 底部动作条：始终贴在滚动视口底部、不随内容 / 窗口缩放 / 滚动漂移；
          负的左右 / 下外边距抵消 AppShell.Main 的内边距，使其横向铺满、紧贴底缘；
          顶部描边 + 背景 + 内边距与内容区分隔，避免遮挡正文。
          一次提交三处写入：PATCH /settings（即时生效）+ /settings/dynamic（重启生效）+ /protection/config（即时生效）。 */}
      <Box
        data-testid="settings-save-bar"
        style={{
          position: 'sticky',
          bottom: 0,
          zIndex: 1,
          marginInline: `calc(-1 * var(--mantine-spacing-sm))`,
          marginBottom: `calc(-1 * var(--mantine-spacing-sm))`,
          padding: `var(--mantine-spacing-sm)`,
          backgroundColor: 'var(--mantine-color-body)',
          borderTop: '1px solid var(--mantine-color-default-border)',
        }}
      >
        <Group>
          <Button
            leftSection={<IconDeviceFloppy size={16} />}
            onClick={handleSaveAll}
            loading={saving}
          >
            {t('common:save')}
          </Button>
          {saved && (
            <Text c="green" size="sm">
              {t('saveBar.savedHint')}
            </Text>
          )}
        </Group>
        {saveError && (
          <Box mt="sm">
            <ErrorAlert message={saveError} />
          </Box>
        )}
      </Box>
    </Stack>
  );
}
