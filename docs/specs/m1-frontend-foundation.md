# 功能规格：前端工程基座、devmock 与 wiki 验收站（M1-FR-035 / 036 / 044）

> 状态：开发中（FR-044 已实现并过验收，待发版标已交付；035/036 骨架已随 044 落地，充实与验收在阶段 1–2）　·　关联 PRD：M1-FR-035、M1-FR-036、M1-FR-044　·　分支：main 直接提交（044 先行，035/036 随阶段推进充实）

## 1. 背景与目标

属 M1。FR-044 是 ROADMAP 阶段 0 的最后一个工程基座项：把四个前端应用工程（web/ui/wiki/devmock）从占位 package.json 变成可构建、可测试、被严格静态检查约束的真实工程，并把前端门禁接进 Docker 质量门。FR-035（devmock）与 FR-036（wiki）的**工程骨架**随 FR-044 一并建立，其**内容充实与验收**分别落在 ROADMAP 阶段 1–2（依据 ADR-0014 前端先行工序）。

## 2. 需求（要什么）

**FR-044 前端工程基座与严格静态检查接线（阶段 0）**

- `packages/typescript-config`：真实共享 TS 配置（`base.json` 与浏览器应用配置），锁 `strict`、`noUncheckedIndexedAccess`、`isolatedModules`、`noUnusedLocals`、`noUnusedParameters`、`noFallthroughCasesInSwitch`。
- `packages/eslint-config`：真实共享 ESLint 9 扁平配置（typescript-eslint 严格集 + react-hooks + eslint-config-prettier 消除冲突），消费侧一律 `--max-warnings 0`。
- 四个应用工程脚手架（Vite + React + TS，消费上述共享配置），各自具备 `typecheck` / `lint` / `test` / `build` 四个 scripts 并可独立运行：
  - `packages/ui`：Tailwind + shadcn/ui 基座，首批引入 1–2 个基础组件源码（如 Button）以证明「组件源码归 ui、被 wiki/web 消费」链路；
  - `apps/wiki`：自建轻量预览站壳（Vite 应用，不引入 Storybook），含首批组件的预览页；
  - `packages/devmock`：MSW 双端骨架——浏览器 worker 产物 + Node 启动器，handlers 暂为空集；
  - `apps/web`：应用壳（入口 + 空首页），页面与路由属阶段 2。
- 质量门接线：容器内生成并提交 `pnpm-lock.yaml`（宿主禁 pnpm）；`quality-gate.sh` 前端段在 install 后增加 `prettier --check`（根配置，只检不改），随后 `turbo typecheck lint test build`；组件测试框架为 Vitest + Testing Library（jsdom）。
- 范围内：以上工程与配置、根 Prettier 配置、质量门脚本前端段调整、锁文件入库。
- 不做（范围外）：web 业务页面与路由（阶段 2）；OpenAPI 类型生成与契约比对接线（阶段 1，依赖 utoipa 导出）；浏览器 E2E（Playwright，随阶段 2 页面引入）；Storybook。

**FR-035 devmock（骨架随 044，充实与验收在阶段 1–2）**

- 双端运行：浏览器 MSW worker（供 web 显式开关连接）与 Node 端（供测试与独立运行）。
- 契约一致：请求/响应模型来自 utoipa OpenAPI 生成的 TS 类型（阶段 1 接线 openapi-typescript），契约比对测试断言 handlers 与 OpenAPI 端点集一致，防手工漂移（ADR-0012）。
- 有状态内存数据 + 场景包，至少：默认数据、空实例、权限受限、上游失败、迁移进行中、迁移部分失败、大仓库分页（分页量按 NFR-16 覆盖 > 2 整页）。
- web 仅在显式开发开关（构建期 `VITE_MOCK` + 运行时 URL/localStorage）下连接 devmock；生产构建默认关闭且不打包 mock worker 与任何测试凭据。

**FR-036 wiki 验收站（壳随 044，覆盖与验收在阶段 2）**

- 自建预览站：路由化的「组件 × 状态」矩阵页与业务模式页；首页给出组件索引与状态覆盖表。
- 状态覆盖：正常、加载、空数据、错误、无权限、超长文本、大数据量（按组件适用性取子集）。
- 流程约束：新增共享组件必须先在 wiki 建立可复现预览，再进入产品页面；wiki 是视觉回归与无障碍人工检查的稳定入口。

## 3. 设计（怎么做）

- 依赖清单（首批引入，均为前端 devDependencies/dependencies；按规则 15 经本规格评审确认）：react ^19、react-dom ^19、vite ^7、@vitejs/plugin-react、typescript ~5.x、tailwindcss ^4、shadcn/ui 生态（radix-ui、class-variance-authority、clsx、tailwind-merge、lucide-react）、msw ^2、vitest ^3、@testing-library/react、jsdom、eslint ^9、typescript-eslint、eslint-plugin-react-hooks、eslint-config-prettier、prettier ^3。@tanstack/react-table ^8 随表格组件落地引入；openapi-typescript 随阶段 1 契约接线引入；react-router 随阶段 2 页面引入。
- 依赖方向不变量：ui 不依赖 web；web/wiki 单向消费 ui；devmock 不被生产构建引用。
- shadcn 组件经 CLI 以源码形式归入 `packages/ui`（copy-in，非运行时依赖），四层组织（基础/复合/布局/业务模式，ADR-0011）。
- turbo 任务沿用现有 `turbo.json`（typecheck/lint/test/build）；配置包（eslint-config/typescript-config）只被消费、不要求四 scripts。
- 质量门（`scripts/quality-gate.sh`）前端段最终形态：`pnpm install --frozen-lockfile` → `pnpm prettier --check .` → `pnpm turbo typecheck lint test build`；锁文件入库后该段自动启用（现有防漏检护栏保持）。
- 相关裁决引用：ADR-0011（UI 设计系统）、ADR-0012（契约真源与比对）、ADR-0014（前端先行工序）。

## 4. 任务拆分

- [x] FR-044：共享配置包真实化（typescript-config / eslint-config）+ 根 Prettier 配置
- [x] FR-044：四应用工程脚手架 + ui 首批 shadcn 组件（容器内 CLI 拉取 Button，官方源为 radix-ui 统一包、导入改相对路径供源码消费）+ wiki 预览矩阵 + devmock 双端骨架（MSW 谓词带 * 通配源站，Node 端无 baseURI 不匹配相对路径）
- [x] FR-044：容器内生成并提交 pnpm-lock.yaml；quality-gate.sh 前端段补 prettier 检查
- [x] FR-044 验收（2026-07-12）：16 个 turbo 任务真实执行全绿；红→绿证据三组（Prettier 拦截真实格式违规、TS 未用变量 exit 2、ESLint any exit 1）；wiki 产物含 Button 标记；devmock worker 随构建产出；宿主 target/node_modules/.pnpm-store/.turbo 全为空挂载点
- [ ] FR-035（阶段 1–2）：openapi-typescript 类型生成接线、有状态数据模型、七类场景包、web 显式开关、生产构建剔除证明、契约比对测试
- [ ] FR-036（阶段 2）：第一期共享组件预览页与状态矩阵全覆盖、业务模式页、组件先预览后进产品页的流程落地
- [ ] 文档同步：PRD 状态流转、ARCHITECTURE §2.1（如结构细化）、CHANGELOG 未发布段

## 5. 验收标准

**FR-044（阶段 0 验收，全部经 Docker 质量门产出证据）**

- `pnpm-lock.yaml` 入库；`make check` 前端段真实执行且全绿（install → prettier --check → turbo typecheck/lint/test/build）。
- turbo 输出证明四个应用工程的四类任务**真实运行而非空转**（每工程至少各 1 个真实测试用例）。
- 红→绿证据（各引入一处违规、门变红、移除转绿）：TS 违规（未用变量触 `noUnusedLocals`）、ESLint 违规（触严格集规则）、Prettier 格式违规。
- `packages/ui` 至少一个 shadcn 基础组件源码归入并被 wiki 预览页真实渲染（容器内 build 通过 + wiki 构建产物包含该页）。
- FR-036（2026-07-13）：ui 四层组件落地——基础层 12 个 shadcn 组件（导入改相对路径）、复合层 StatusBadge/Pagination/DataTable（TanStack Table）、状态层 EmptyState/ErrorState/LoadingState+TableSkeleton/ForbiddenState、布局层 AppShell/PageHeader；wiki 重写为 21 组件 × 状态矩阵（正常/加载/空/错误/无权限/超长文本/大数据量）+ 组件索引与覆盖表；ui 34 测试 + wiki 4 测试全绿；测试基础设施补 testing-library cleanup（修跨用例 DOM 累积）。
- devmock Node 端可启动并响应探测请求（骨架级）；浏览器 worker 文件随构建产出。
- 宿主目录零 node_modules 内容（命名卷空挂载点豁免）；前端构建产物只落容器卷或被忽略目录。

**FR-035（阶段 2 验收口径，需用户确认）**

- 七类场景包逐项可切换且行为符合定义；大仓库分页场景数据量 > 2 整页。
- 契约比对测试：devmock handlers 端点集与 OpenAPI 一致，缺失/多余端点即失败。
- web 生产构建产物经检查不含 mock worker 与任何测试凭据（构建产物扫描证据）。

**FR-036（阶段 2 验收口径，需用户确认）**

- 第一期全部共享组件在 wiki 有预览页，状态矩阵按适用性覆盖齐全；首页索引与覆盖表准确。
- 抽样组件经键盘操作与焦点可见性人工检查通过（wiki 作为检查入口）。

## 6. 风险 / 待定

- **版本代际（开发前确认）**：推荐新代际 React 19 + Tailwind 4 + Vite 7（shadcn/ui 主线支持组合）；如需保守回退 React 18 + Tailwind 3 代际，在 FR-044 开工前提出，影响共享配置与组件引入方式。
- 不引入 Storybook（自建轻量预览站，零重型依赖）；若后续确需其插件生态，按依赖确认流程另行评估。
- devmock 契约比对依赖阶段 1 的 OpenAPI 导出；FR-044 阶段 devmock 仅为双端骨架，不以空 schema 造占位契约。
- pnpm-lock.yaml 必须在容器内生成（宿主禁 pnpm）；首次 install 后提交锁文件，前端门禁即自动启用。
