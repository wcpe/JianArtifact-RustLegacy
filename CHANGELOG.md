# 变更日志

本项目所有重要变更记录于此。

格式遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

## 未发布版本

### 新增
- 实现开发中的 M1-FR-046 首启管理员初始化：环境变量自举优先，空用户库回退到数据库同目录原子发布的一次性令牌与 `/setup#token=<令牌>` 页面，提供 setup 状态/提交 API、devmock 场景和真实浏览器链路，并以原子空表判定保证并发仅一个成功；部署脚本按实际数据库路径定位令牌，并在服务明确仍需初始化时才输出一次性链接。
- 实现开发中的 M1-FR-047 本地口令重置 CLI：支持终端隐藏双输与 `--password-stdin` 单行输入，拒绝命令行明文口令，以路径 API 只打开既有 SQLite，并仅更新用户口令哈希与递增会话版本。
- 冻结 Nexus 全量迁移施工契约与 ADR-0017：十态任务生命周期、四态业务阶段、在线显式切换门、三类冲突决策、临时凭据零持久化，以及只支持目录树的离线包 v1 canonical digest 与内容寻址布局。
- 建立迁移领域纯函数基础：状态机按内部检查点重试并以完整校验证据计算成功/部分完成，冲突策略校验批内源与目标唯一性，离线包模型严格校验版本、路径、快照绑定、仓库字段组合、分片计数及 group 引用；devmock 同步来源切换门和计划缺口的 `partial` 语义。
- 实现 Nexus 迁移后端主链路：在线 REST、原生数据目录和离线目录包三来源发现，不可变计划与 SQLite 任务/检查点持久化，Raw/Maven/npm hosted/proxy/group 目标写入，暂停/取消/重试、显式切换与最终增量扫描，以及分仓报告和在线漂移证据。
- 完成 npm 迁移协议完整性校验与报告读链：每个 canonical tarball 必须匹配唯一有效 packument 和 manifest 版本，非法或悬空 dist-tag、缺失/畸形 packument 均计为协议失败；报告 API 与 Web 展示 blob、协议元数据、proxy 正缓存及在线漂移计数。
- 增加同二进制 `migration export` CLI：从迁移工作目录内的 Nexus 原生文件型 blob store 与 canonical 仓库配置生成离线包 v1，以 `.partial`、源快照复核和读取器自校验防止发布不完整目录。

## [0.2.0] - 2026-07-15

### 新增
- 建立工程骨架：Cargo / pnpm / Turborepo 工作区、Docker 化统一质量门与 CI 工作流。
- Makefile 统一构建入口（check / release / clean）与容器内 Linux 发布构建：产出 x86_64 tar 发布包（含二进制、用户级 systemd unit 模板、VERSION 与 sha256 校验和），构建前强制校验 VERSION 与 Cargo 版本一致。
- rootless 部署与回滚（make deploy / rollback）：SSH 上传经 sha256 校验后落 releases 目录并以 current 符号链接原子切换，用户级 systemd 全程无 root；健康确认失败自动回滚上一发布，保留份数可配置；部署逻辑测试（桩 systemd）纳入质量门。
- 前端工程基座（React 19 + Tailwind 4 + Vite 7）：web / ui / wiki / devmock 四工程脚手架与共享严格配置（TS strict + noUncheckedIndexedAccess、ESLint 9 严格集 --max-warnings 0、Prettier 只检不改）接入 Docker 质量门；ui 经 shadcn CLI 收录 Button 组件源码，wiki 提供组件预览矩阵，devmock 建立 MSW 双端骨架。
- 工作区结构调整为 apps/（可运行入口）+ packages/（共享库）双根（ADR-0016）。
- 后端服务基座与认证域契约骨架：axum 服务（配置加载/中文日志/SQLite 迁移框架/统一错误结构/请求标识）、健康检查与就绪自检（/healthz、/readyz 真实现）、登录/刷新/退出/API Token/用户管理契约端点（utoipa OpenAPI 唯一真源，GET /api/v1/openapi.json 与 `jianartifact openapi` 导出）；cargo-audit 与 cargo-deny 许可证检查接入质量门。
- 仓库域与迁移域契约骨架：仓库 CRUD（删除双模式 + 确认头）、ACL、浏览/搜索/使用片段；迁移三类来源、发现、计划与冲突、十态任务生命周期、分仓报告——当前管理 API 契约共 34 个 operation；契约快照 `packages/devmock/openapi.json` 新鲜度校验与 devmock 无幻影端点比对测试纳入质量门（make openapi 重新生成）。
- devmock 七类场景包与有状态内存模型（M1-FR-035）：认证/Token/用户/仓库/浏览/迁移全语义、契约双向硬断言（无幻影 + 全量覆盖）；openapi-typescript 契约类型生成接线。
- 共享组件库四层（M1-FR-036）：基础（shadcn 源码 12 组件）、复合（StatusBadge/Pagination/DataTable 基于 TanStack Table）、状态（Empty/Error/Loading/Forbidden）、布局（AppShell/PageHeader）；wiki 重写为组件 × 状态矩阵验收站。
- 管理端六组页面对 devmock 开发：登录/仪表盘/仓库列表与详情（文件下钻、使用片段）/API Token（一次性明文）/用户管理/迁移向导（源配置→发现→计划→执行→轮询进度→报告，task_id 持久化刷新可找回，M1-FR-034 向导 UI）；三层守卫路由 + 统一 API client；VITE_MOCK 构建期常量守卫保证生产产物零 mock 与测试凭据。
- 认证与用户域后端真连（M1-FR-001~006、045）：管理员原子自举、Argon2id 登录、JWT HS256 会话刷新/退出、API Token、Basic/Bearer 提取器、用户管理与最后 Admin 原子保护全部落地；新增 `GET /api/v1/auth/me` 作为前端权威角色来源，改密/重置通过 `token_version` 撤销全部既有会话，自改口令用哈希与版本 CAS 防止并发管理员重置被旧快照覆盖；JWT 引入 sid 会话家族，刷新沿用、退出整族撤销，同一旧 JWT 并发刷新仅一个成功且刷新/退出竞态不遗留有效令牌；Argon2 哈希/校验移交阻塞线程池，不存在用户仍执行同参数固定哈希校验；devmock 绑定请求 Bearer 会话并统一 `AUTH_` 错误码，管理端每次恢复都经 `/auth/me` 复核身份。
- Playwright 真实浏览器 E2E（14 用例，容器内 chromium）：登录鉴权、建仓与删除、仓库详情、Token 一次性明文、用户管理、迁移向导流程与刷新找回、改密重登，跑在 devmock 模式下并纳入质量门第 9 步。
- 阶段 3 真连浏览器通道：默认开发经 Vite 代理连接真实 Rust，只有 `VITE_MOCK=true` 才启用 devmock；新增隔离 SQLite/数据目录的 Playwright 配置，覆盖登录、会话恢复、Token、用户、改密与旧口令失效，并验证 `/favicon.ico` 返回 200、密码输入框 autocomplete 合规且无相关控制台警告。
- 仓库与格式协议契约纠偏：ACL 改用精确用户名路径键，PATCH 支持显式 null 清空上游凭据引用，FileEntry 增完整 path，仓库创建/编辑补齐类型专属字段；`/repository/{name}/{path}` 的 GET/HEAD/PUT/DELETE 纳入统一 OpenAPI，devmock 双向契约只覆盖 `/api/v1` 管理业务路径。
- 仓库核心真实后端（M1-FR-007~009、021 批次 1）：新增 SQLite 仓库/ACL/group 成员迁移、无 IO 授权纯函数与仓库领域服务，仓库 CRUD/PATCH/删除、精确用户名 ACL、可见性过滤、停用管理面、group 成员防泄露及类型不变量全部替换 501；`/readyz` 改为 SQLite 临时事务回滚与随机排他文件探针，运行期挂载丢失不自愈；管理端补 Maven 版本策略、proxy 凭据显式清空与精确用户名 ACL，真实 Rust Playwright 覆盖仓库创建、更新、ACL、409、404 和确认删除。
- BlobStore、制品索引与 Raw 真实协议（M1-FR-010/011/013/014/019）：新增 SHA-256 内容寻址存储、四摘要流式计算、提交守卫/启动恢复、同摘要读写租约，以及与索引删除/替换同事务持久化的回收意图；回收代次隔离陈旧任务，`mode=config` 取消意图并保留孤儿 blob，请求取消或进程退出后的未完成回收由启动恢复幂等收口。仓库统计、目录浏览、路径搜索及 config/data 删除双模式接入 SQLite 真源。Raw hosted 支持流式上传下载、HEAD、覆盖策略、删除和 JSON 目录；Raw proxy 支持先缓存后响应、32 并发单飞、禁重定向、运行时 Basic/Bearer 凭据引用、上游错误分类与 URL 变更事务失效缓存；单飞按 `upstream_url` 配置快照隔离，缓存事务条件核对预期 URL，拒绝旧配置提交后按新配置最多重试一次，连续变化返回稳定 503；OpenAPI 与 devmock 类型同步更新。
- 口令修改与重置（M1-FR-045）：`POST /api/v1/auth/password` 修改本人口令（旧口令验证、旧口令立即失效）与用户管理重置口令（`PATCH /api/v1/users/{id}` 扩 password 字段）；顶栏修改口令模态框、用户管理行内重置模态框；devmock 同步行为与测试。
- 仓库详情升级为左侧文件树 + 右侧详情面板（M1-FR-019 验收细化）：目录懒加载展开、文件元数据卡（大小/sha256 可复制/更新时间）、目录内容表与树联动选中。
- 仓库管理页面按参考布局改造（M1-FR-008/009/019）：列表页扩制品数/总大小/状态/上游/操作列（设置直达配置页签、删除经模式选择 + 输名确认的高危对话框）；详情页重组为浏览/配置/权限三页签（页签映射 URL 锚点）——浏览页签文件详情补内容类型、四算法校验和卡（SHA-256/SHA-1/MD5/SHA-512 可复制）与下载按钮，配置页签提供仓库编辑（可见性/覆盖策略/上游/成员/停用）与危险区删除，权限页签提供 ACL 增删管理（403 显示无权状态）；契约 RepoInfo 扩制品数与总大小、FileEntry 扩多校验和与内容类型，devmock 增格式端点下载 mock。
- 制品详情格式感知与主题统一（M1-FR-019/020）：浏览页签改为面板化左树右详情（目录展开箭头、蓝色文件夹图标、选中高亮）；Maven 详情按 GAV 推导坐标卡并给出 Maven/Gradle/Gradle KTS 依赖片段，npm 详情按包名版本推导并给出 npm/yarn/pnpm 安装命令，校验和按格式惯例排序；仓库接入片段扩展 Maven 的 build.gradle/build.gradle.kts 与 npm 的 .yarnrc.yml；配置页签表单改双列网格、危险区收窄为单行；管理端主色统一为蓝色系。
- 迁移向导体验升级（M1-FR-034 验收细化）：步骤映射 URL 锚点 + 每步中间态缓存（刷新/直达可恢复，损坏安全回退，口令绝不缓存）、步骤条可回退；在线拉取凭据改为直接输入 Nexus 账号口令（仅传输与内存使用，不落库不进日志与报告），离线数据目录的 Nexus 地址选填（留空走手动补配置）；计划页逐仓库映射明细表；进度页暂停/继续/取消/重试控制与中断态提示。
- 管理端控制台外壳与列表能力对齐旧项目观感（M1-FR-004/006/019）：布局改为 Mantine 蓝主题 + 旧 logo，侧栏分组导航可折叠成图标 rail（文字淡出不重排）、左下开源许可入口，页眉搜索框左对齐（Ctrl K 聚焦）+ 右侧用户头像下拉菜单（修改口令 / 退出登录）；整体固定视口、导航与内容各自内部滚动，仓库详情浏览区左树右详情各自内滚（点目录仅展开、选中文件才显示详情）；DataTable 分页栏固定内容页底部、表格区内部滚动；仓库 / 用户 / Token 列表端点新增可选 `q` 名称子串筛选参数（契约扩展），三页加带 300ms 防抖的名称筛选框；devmock 新增 stress 压测场景（100 仓库 × 1 万文件、1000 Token、100 用户）供分页 / 筛选 / 性能压测。
- 顶栏「刷新当前页」按钮与迁移向导 loading/进度/恢复完善（M1-FR-034）：内容页各自注册 reload，顶栏刷新按钮一键重载本页数据（无可刷新内容时禁用、刷新中图标旋转）；迁移向导发现 / 生成计划 / 创建任务三步以处理态卡片明确展示进度，执行页按十态状态给出中文阶段说明并对齐控制按钮（补 queued/pausing 取消），刷新或重新进入按在途任务 / 锚点 + 缓存恢复到正确步骤，在途任务已不存在时进度页提供「重新开始」逃生出口。
- 管理端六组页面对 devmock 开发（M1-FR-034 向导 UI 等）：登录/仪表盘/仓库列表与详情（文件浏览+使用方式）/API Token（明文一次性展示）/用户管理/迁移向导（源配置→发现→计划→执行→进度轮询→报告，task_id 落 localStorage 刷新可找回）；三层守卫路由 + 会话上下文 + 契约类型 API client；mock 开关经构建期常量守卫实现生产产物零 mock 代码零测试凭据（产物 grep 为证）。

### 修复
- 修复 Windows Git Bash 将远端 Linux `DEPLOY_DIR` 绝对路径改写为本机盘符路径的问题，确保 SSH/systemd 部署原样使用目标机路径。
- 限制前端质量门的 Turborepo 并发度，避免容器内多个检查任务因峰值内存争用被系统终止。
- 将 devmock Playwright E2E 固定为单 worker 并把单用例超时调整为 120 秒，消除 Docker Desktop 绑定挂载冷缓存下并发 Vite 转换导致的 `page.goto` 假失败；冷缓存 15 用例与完整 14 步质量门均已通过。

> 发版时把"未发布版本"段切成 `## [X.Y.Z] - YYYY-MM-DD`，再新建空的"未发布版本"段。
