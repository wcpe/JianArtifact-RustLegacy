// settings 命名空间文案（FR-111，待各页迁移时填充）。
export default {
  // 页标题（含 loading 兜底分支）
  pageTitle: '设置',
  // 左侧分节导航容器无障碍标签
  navAriaLabel: '设置分节导航',
  // 左侧锚点导航各节标签（与右侧分节标题共用同一真源；防护配置文案归 protection ns，不在此处使用）
  nav: {
    proxy: '网络代理',
    limits: '限制与配额',
    observability: '可观测性',
    vuln: '漏洞库',
    auth: '安全 / 会话',
    protection: '防护配置',
  },
  // 各「重启生效」节共用的徽标
  restartHint: '保存后重启生效',
  // 网络代理节
  proxy: {
    title: '网络代理',
    desc: '统一出站代理（回源 / 迁移 / 漏洞库 / OIDC / 在线更新共用）。单一通用代理对全部出站请求生效；可填用户名 + 密码，用户名回显、密码不回显（留空保留现有），URL 留空表示不配置代理。保存后**即时生效、无须重启**。',
    // 单一通用代理槽（FR-146）
    generalTitle: '通用网络代理',
    // 支持的 URL scheme 说明（socks5h 由代理端解析域名，受污染网络推荐）
    schemeHint:
      '支持 http:// / https:// / socks5:// / socks5h://（socks5h 由代理端解析域名，受污染网络推荐）',
    noProxyLabel: '直连绕过（no_proxy）',
    // 槽位「测试」按钮（FR-128 回流 UX 改造）
    testButton: '测试',
    // 「清除代理」按钮：清空槽位的 URL / 用户名 / 密码，不动 no_proxy。
    clearProxy: '清除代理',
    proxyWillClear: '保存后清除该代理',
    // 模态框（槽位「测试」按钮 → 弹出模态框 → 输入 URL → 测试 → 显示结果）
    testModalTitle: '连通性测试',
    closeTestModal: '关闭连通性测试',
    testModalDesc: '测试将使用表单中当前填写的代理配置（无需先保存）访问目标 URL。默认 10 秒超时。',
    testUrlLabel: '目标 URL',
    testUrlPlaceholder: 'https://www.google.com',
    testRunButton: '测试',
    testTesting: '测试中…',
    // 测试按钮旁的辅助按钮（UX 完善：避免用户手敲默认 URL）
    resetUrl: '重置',
    // 测试结果展示
    testResultOk: '连通（{{status}} · {{elapsed}}）',
    testResultFail: '失败：{{error}}（{{elapsed}}）',
    testResultFailNoError: '连接失败',
    testUrlRequired: '请填写目标 URL',
    // 错误区辅助
    timeout: '请求超时',
    copyError: '复制错误',
  },
  // 单代理三字段子组件（ProxyFields）
  proxyFields: {
    passwordConfigured: '密码已配置',
    passwordWillClear: '保存后清除密码',
    urlLabel: 'URL',
    usernameLabel: '用户名',
    usernamePlaceholder: '可选',
    passwordLabel: '密码',
    passwordPlaceholder: '留空保留现有密码',
    clearPassword: '清除密码',
  },
  // 限制与配额节
  limits: {
    title: '限制与配额',
    maxArtifactSizeLabel: '单个制品上传上限（字节）',
    maxArtifactSizeDesc: '留空表示不额外限制；超限上传返回 413。',
    maxArtifactSizePlaceholder: '不限制',
  },
  // 可观测性节
  observability: {
    title: '可观测性',
    auditRetentionDays: '审计日志保留天数',
    auditMaxRows: '审计日志行数上限',
    usageDetailEnabled: '记录逐条访问 / 下载明细（使用分析）',
    usageMaxDetailRows: '使用明细行数上限',
    metricsEnabled: '启用 Prometheus 指标端点（/metrics）',
    metricsAllowAnonymous: '允许匿名抓取 /metrics（须限内网 / 反代后）',
    timeseriesEnabled: '启用指标时序采集',
    timeseriesSampleInterval: '时序采样间隔（秒）',
    timeseriesRetentionDays: '时序保留天数',
  },
  // 漏洞库节
  vuln: {
    title: '漏洞库',
    enabled: '启用漏洞库离线镜像',
    sourceBaseUrl: '镜像数据源基址',
    refreshInterval: '刷新周期（秒）',
    downloadTimeout: '下载超时（秒）',
  },
  // 安全 / 会话节
  auth: {
    title: '安全 / 会话',
    desc: '仅会话 / 登录锁定可调标量；OIDC / LDAP 等密钥项不在此处、只能经配置文件 / 环境变量设置。',
    sessionTtl: '会话有效期（秒）',
    loginMaxFailures: '触发锁定的连续失败次数',
    loginLockoutSecs: '锁定时长（秒）',
  },
  // 底部全局保存条
  saveBar: {
    savedHint: '已保存。代理即时生效；限制配额 / 可观测性 / 漏洞库 / 安全会话重启后生效。',
  },
} as const;
