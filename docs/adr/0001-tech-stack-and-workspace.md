# ADR-0001：技术栈选型与单一工作区布局

## 状态
已接受（「单一 `apps/` 根、禁止 `packages/`」一条经 ADR-0016 取代为 `apps/` + `packages/` 双根结构；本 ADR 的技术栈选型等其余决策仍有效）

## 背景
项目从零重写一个自托管制品仓库，第一期需交付多用户认证、hosted/proxy/group 仓库、Raw/Maven/npm 三种格式协议与 Nexus 全量迁移。必须在开工前一次性确定后端语言框架、前端技术栈与仓库组织方式，避免中途更换基础设施。需求同时给出硬约束：所有可构建子工程统一放在 `apps/*` 下，禁止出现 `packages/*`、`services/*` 等第二套子工程根目录。

## 决策
- 后端沿用已验证的组合：axum 0.8 + tokio 异步运行时 + sqlx（SQLite，bundled 静态链接）+ argon2（密码哈希）+ jsonwebtoken（HS256 会话签名）+ reqwest（rustls TLS）。全链路使用 rustls 与 RustCrypto 生态，零 C 依赖；crate 级声明 `unsafe_code = "forbid"`。
- 前端采用 React + TypeScript strict + Vite 构建；组件方案见 ADR-0011。
- 工作区采用 pnpm workspace + Turborepo 编排，子工程固定为 `apps/{server,web,ui,wiki,devmock,eslint-config,typescript-config}`；Rust 侧以 Cargo workspace 管理 `apps/server`。

## 理由
- axum + tokio + sqlx 组合在上一代实现中已验证可支撑流式上传下载、代理回源与迁移任务，沿用可最大限度降低重写期的技术风险。
- 全 rustls/RustCrypto 与 SQLite bundled 静态链接使编译与部署不依赖系统 OpenSSL/sqlite3，支撑单二进制、低运维的产品定位（见 ADR-0013）。
- `forbid(unsafe)` 在编译期杜绝生产代码引入 unsafe，与需求「Rust 生产代码禁止 unsafe」直接对应。
- 单一 `apps/*` 根目录 + pnpm + Turborepo 使质量门（build/lint/test/typecheck）可按子工程增量编排，架构依赖方向可静态检查。

## 后果
- 正面：技术风险前置收敛；构建产物为纯静态链接二进制；工作区结构对架构门禁与增量构建友好。
- 负面与约束：绑定 SQLite 单机模型，多节点方案留待后续独立 ADR；axum 大版本升级需评估 tower 中间件生态兼容性；所有新增子工程（含未来内部工具）必须进入 `apps/*`，不得另起子工程根目录。

## 备选方案
- actix-web：性能同级，但与 tower 生态整合弱于 axum，且无本项目沿革中的已验证经验。
- rusqlite（同步驱动）：进入异步运行时需自行封装线程池；sqlx 的连接池与异步接口开箱即用。
- 全新组合重新选型：违背「沿用已验证设计降低重写风险」的原则，收益不明确。
- 多仓库拆分（server/web 各自独立仓库）：契约同步、原子提交与统一质量门成本高，不适合单团队单产品形态。
