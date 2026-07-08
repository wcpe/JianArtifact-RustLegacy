// Nexus 迁移管理页文案命名空间（FR-111）。
export default {
  // 页头
  title: 'Nexus 迁移',
  intro:
    '从源 Nexus OSS 迁移仓库与制品：在线 REST 或离线 blob store 预览 → 勾选 → 执行 → 查看报告。离线迁移只依赖本地 blob store 路径；在线迁移凭据仅以引用名提供，明文不入库、不回显。',

  // 步骤条（Stepper.Step 的 label / description）
  step: {
    sourceLabel: '选源与加载',
    sourceDesc: '在线填源地址，离线填本地路径',
    selectLabel: '勾选仓库',
    selectDesc: '选仓库、改名、补填推断项',
    planLabel: '确认计划',
    planDesc: '核对执行计划后开始',
    reportLabel: '执行与报告',
    reportDesc: '查看进度与结果',
  },

  // 源形态切换（SegmentedControl）
  mode: {
    online: '在线（REST API）',
    offline: '离线（blob store）',
  },

  // 迁移方式切换（SegmentedControl）
  method: {
    online: '在线拉取（HTTP 下载）',
    offline: '离线目录（blob store）',
  },

  // 源配置输入项
  source: {
    baseUrlLabel: '源 Nexus 地址',
    authRefLabel: '凭据引用（auth_ref，可选）',
    authRefDesc: '仅填引用名；真实凭据由后端 env 解析，明文不入库、不回显。匿名源可留空。',
    authRefPlaceholder: '例如 NEXUS_SRC',
    offlinePathLabel: '离线 blob store 路径',
    offlinePathDesc: '服务进程可访问的本地 Nexus 文件型 blob store 根目录。',
    offlineBaseUrlLabel: '源 Nexus 地址（可选，权威通道）',
    offlineBaseUrlDesc:
      '可选填源 Nexus REST 地址；可达时取权威仓库配置（格式 / 类型 / 上游），否则回退按内容路径推断。',
    offlineBaseUrlPlaceholder: 'https://nexus.example（留空则纯离线推断）',
    migratePathLabel: '离线 blob store 路径（制品本体来源）',
    migratePathDesc: '搬运需从离线 blob store 读取制品本体，其下应含 content/ 子目录。',
    targetRepoAria: '{{name}} 目标仓库名',
    targetRepoPlaceholder: '目标名（默认 {{name}}）',
  },

  // 阶段中文标签（OnlinePullJob.phase）
  phase: {
    enumerating: '枚举资产中',
    downloading: '下载搬运中',
    paused: '已暂停',
    cancelled: '已取消',
    done: '已完成',
    failed: '失败',
  },

  // 预览 / 索引加载结果区
  preview: {
    button: '预览仓库',
    // 离线入口：加载仓库索引（双通道）
    indexButton: '加载仓库索引',
    next: '下一步：勾选仓库',
    count: '可迁移仓库（{{count}}）',
    thRepo: '仓库',
    thFormat: '格式',
    thDetail: '类型 / 内容',
    thSource: '格式来源',
    thType: '类型',
    thBlobCount: 'blob 数',
    thSize: '大小',
    thUpstream: '上游',
    // 离线预览行的内容文案（blob 数量）
    blobCount: '{{count}} 个 blob',
    // 格式来源徽章（FR-141 双通道）
    sourceAuthoritative: '权威',
    sourceInferred: '推断',
    // FR-124 离线预览异步化：枚举任务超时 / 失败提示
    timeout: '离线预览任务超时（blob store 可能过大），请稍后重试或缩小目录',
    failed: '离线预览枚举失败，请检查 blob store 路径与目录结构',
    // FR-141 索引加载超时 / 失败
    indexTimeout: '仓库索引加载超时（blob store 可能过大），请稍后重试或缩小目录',
    indexFailed: '仓库索引加载失败，请检查 blob store 路径与目录结构',
  },

  // 勾选与执行区
  select: {
    empty: '请先在上一步预览仓库。',
    method: '迁移方式',
    chosen: '勾选要搬运的仓库（已选 {{count}}）',
    onlineHint:
      '在线拉取仅对 maven2 hosted 仓库有效，经源 Nexus REST 枚举并逐个 HTTP 下载制品，无需本地 blob store 目录；非 maven / 非 hosted 的所选仓库会被跳过并列入报告。 每个仓库可选填目标仓库名，留空即与源同名。',
    prevStep: '上一步',
    runOnline: '执行在线拉取',
    onlineFootnote:
      '在线拉取建仓 + 经 HTTP 下载同步制品；仅 maven2 hosted 仓库被处理，其余进报告 的整仓跳过列表。',
    runProxy: '执行 proxy 搬运',
    runHosted: '执行 hosted 搬运',
    activeMigrationBlocked: '已有迁移任务在途，请在任务中心或本页续看',
    offlineFootnote:
      '离线搬运只读取本地 blob store，不需要源 Nexus 地址或凭据；proxy 搬运缓存制品，hosted 搬运完整制品。',
    // FR-141 推断项补填：类型由管理员选，proxy 需补上游
    inferredHint:
      '「推断」项的仓库类型由内容路径猜得，请为勾选的推断项确认类型（proxy 需补上游）。',
    typeLabel: '类型',
    typeAria: '{{name}} 仓库类型',
    typeProxy: 'proxy',
    typeHosted: 'hosted',
    typePlaceholder: '选类型',
    upstreamAria: '{{name}} 上游地址',
    upstreamPlaceholder: 'proxy 上游 URL（可选）',
    nextPlan: '下一步：确认计划',
    inferredNeedsType: '有勾选的推断仓库尚未确认类型，请先补填后再继续。',
  },

  // 执行计划确认区（FR-142）
  plan: {
    title: '执行计划',
    intro: '核对以下计划后开始迁移。目标已存在且 sha256 一致的制品会被幂等跳过。',
    thSource: '源仓库',
    thTarget: '目标仓库',
    thMethod: '方式',
    thType: '类型',
    thUpstream: '上游',
    methodOnline: '在线拉取',
    methodProxy: 'proxy 搬运',
    methodHosted: 'hosted 搬运',
    prevStep: '上一步',
    start: '开始迁移',
    empty: '未选择任何仓库。',
    upstreamNote: '上游仅作展示与运维参考，不随本次搬运下发（P1 建仓后由运维配置）。',
  },

  // 迁移历史区（FR-140 持久化）
  history: {
    title: '迁移历史',
    refresh: '刷新',
    empty: '暂无迁移历史。',
    loadError: '加载迁移历史失败',
    thTask: '任务',
    thState: '状态',
    thStarted: '开始时间',
    thFinished: '完成时间',
    view: '查看报告',
    rerun: '重跑续传',
    rerunStarted: '重跑任务已发起，正在导入',
    detailTitle: '分仓库报告',
    detailError: '加载报告失败',
    kindOnline: '在线拉取',
    kindOfflineProxy: '离线 proxy',
    kindOfflineHosted: '离线 hosted',
    stateRunning: '运行中',
    statePaused: '已暂停',
    stateSucceeded: '已完成',
    stateFailed: '失败',
    stateCancelled: '已取消',
    stateInterrupted: '已中断',
  },

  // 迁移报告区
  report: {
    empty: '尚无迁移报告。',
    title: '迁移报告',
    noRepos: '无仓库被搬运。',
    thRepo: '仓库',
    thFormat: '格式',
    thCreated: '新建仓库',
    thMigrated: '已迁制品',
    thSkipped: '跳过制品',
    createdYes: '是',
    createdExisting: '已存在',
    skippedRepos: '整仓跳过（非目标类型）：',
    backToSelect: '返回勾选',
  },

  // 成功提示（toast）
  toast: {
    offlineDone: '迁移已完成，请查看报告',
    offlineStarted: '离线目录搬运任务已发起，正在后台导入',
    onlineStarted: '在线拉取任务已发起，正在导入',
  },

  // 在线拉取任务进度面板
  job: {
    queueTitle: '在线拉取导入队列',
    resume: '继续',
    pause: '暂停',
    cancel: '取消',
    progressAria: '导入进度',
    progress: '进度 {{done}} / {{total}}（{{percent}}%）',
    byteProgress: '进度 {{done}} / {{total}}（{{percent}}%）',
    pendingTitle: '迁移任务已启动',
    pendingDesc: '任务已启动，正在加载首次进度快照',
    pendingJobId: '任务 ID：{{id}}',
    missingJobId: '后端未返回任务 ID，请稍后重试',
    offlinePreviewTitle: '离线预览',
    offlinePreviewSummary: '离线预览已完成：共 {{repos}} 个仓库、{{blobs}} 个 blob。',
    offlinePreviewNoRepos: '离线预览已完成，但未发现可迁移仓库。',
    indexTitle: '离线仓库索引',
    indexSummary: '仓库索引已加载：共 {{repos}} 个仓库。',
    indexNoRepos: '仓库索引已加载，但未发现可迁移仓库。',
    migrated: '新搬 {{count}}',
    skipped: '已跳过 {{count}}',
    skippedExisting: '已存在跳过 {{count}}',
    skippedFailed: '失败跳过 {{count}}',
    currentRepo: '当前仓库：',
    currentPath: '当前文件：',
    failedTitle: '任务失败',
    noRepos: '无仓库被搬运。',
    thSourceRepo: '源仓库',
    thTargetRepo: '目标仓库',
    thFormat: '格式',
    thCreated: '新建仓库',
    thMigrated: '新搬制品',
    thSkippedExisting: '已存在跳过',
    thSkippedFailed: '失败跳过',
    thSkipped: '跳过制品',
    thBlobCount: 'blob 数',
    summaryTotal: '总数 {{count}}',
    summaryMigrated: '新搬 {{count}}',
    summarySkippedExisting: '已存在跳过 {{count}}',
    summarySkippedFailed: '失败跳过 {{count}}',
    createdYes: '是',
    createdExisting: '已存在',
    skippedRepos: '整仓跳过：',
  },
} as const;
