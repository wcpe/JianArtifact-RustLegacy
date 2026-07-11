# ADR-0016：工作区采用 apps/ + packages/ 双根结构

## 状态
已接受

## 背景
ADR-0001 与初始工程组织约束规定「所有可构建子工程统一位于 `apps/*`，禁止新增 `packages/*` 等第二套根目录」，用意是以单一根避免子工程根目录无序扩张。但落地前端基座后，`apps/` 下混放了两类语义不同的工程：可运行/可部署的**入口应用**（server、web、wiki）与被 import 的**共享库**（ui 组件库、devmock、eslint-config、typescript-config）。二者混在一个根下，语义不清、与 Turborepo 社区通行的 `apps/`（应用）+ `packages/`（库）约定相悖，新协作者需要额外辨认哪个是库哪个是应用。此时前端仅有脚手架、几乎无业务代码，是调整结构的最低成本时机。

## 决策
- **工作区改为恰好两个子工程根**：`apps/`（可运行入口）与 `packages/`（共享库）。
  - `apps/`：`server`（Rust 后端）、`web`（管理端应用）、`wiki`（组件预览站，可运行）。
  - `packages/`：`ui`（共享组件库，含基础/复合/布局/业务模式四层与主题）、`devmock`、`eslint-config`、`typescript-config`。
- **仍禁止第三套根**：不得再新增 `services/`、`libs/` 等根目录；防漂移意图不变，只是把「单一 `apps/` 根」放宽为「`apps/` + `packages/` 两个各有明确语义的根」。
- **取代 ADR-0001 中「单一 apps/ 根、禁止 packages/」这一条**（ADR-0001 的技术栈选型等其余决策不受影响，仅工作区根结构一条被本 ADR 取代）。
- 前 ADR（0011/0012/0014）正文中对 `apps/ui`、`apps/devmock`、`apps/eslint-config`、`apps/typescript-config` 的路径引用，自本 ADR 起相应解读为 `packages/` 下同名包；这些 ADR 的决策语义（组件源码归 ui 包、devmock 契约比对等）不变，正文按不可变原则不改。
- 工作区依赖一律用包名（`@jianartifact/*` 与 `workspace:*`），与物理位置解耦；后端 Cargo workspace 成员 `apps/server` 不变。

## 理由
- `apps/`=部署/运行、`packages/`=被引用，语义一眼可辨，符合 Turborepo 主流约定，降低认知成本。
- 早期结构调整近乎零成本（仅脚手架），越晚搬迁牵动的代码越多。
- 「两个有界的根」与「一个根」在防漂移上等价——关键是根集合有界且语义明确，而非数量必须为一。

## 后果
- 正面：工程语义清晰、贴合社区约定；共享库与应用的依赖方向更直观（packages 不依赖 apps）。
- 负面与约束：一次性目录搬迁与引用同步（pnpm 工作区、compose 卷、忽略与格式化范围、`@source` 相对路径、锁文件 importer 键）；架构不变量与若干文档需随之更新；前 ADR 的历史路径引用需读者按本 ADR 换算。

## 备选方案
- 维持单一 `apps/` 根（ADR-0001 原决策）：语义混淆、与社区约定相悖，被本 ADR 取代。
- 更细粒度拆分（如 ui 再拆成 ui-core/ui-layout/ui-theme 多包）：当前无此复用需求，属过度设计（YAGNI），不采纳；ui 维持单包内四层组织。
- 把 wiki 也归 packages/：wiki 是可运行的预览站而非被 import 的库，归 `apps/` 更契合语义。
