# JianArtifact

> 低运维成本的自托管制品仓库：统一管理 Raw、Maven、npm 的 hosted / proxy / group 仓库，并支持 Nexus Repository OSS 3.70.x 的仓库全量迁移与切换。

## 状态

规格与治理体系已建立，业务实现尚未开始；开发按 [`docs/PRD.md`](docs/PRD.md) 的 M1 功能需求（FR）迭代推进。当前版本 0.1.0。

## 能力（M1 目标）

- 多用户认证与授权：本地用户、Admin/User 角色、API Token、会话即时撤销、仓库级 read/write/admin ACL，匿名只读公开仓库。
- Raw、Maven、npm 三种格式的 hosted、proxy 与只读 group 仓库，保持包管理器原生协议兼容。
- Nexus OSS 3.70.x 仓库全量迁移：在线 / 离线入口、迁移计划预览、幂等增量续传、持久化报告。
- 低运维运行形态：单个 Rust 服务进程 + SQLite 元数据库 + 本地 blob 数据目录，前端静态产物内嵌于单二进制。

## 工程结构

```text
JianArtifact/
├─ apps/                  # 可运行入口
│  ├─ server/             # Rust 后端：管理 API、格式协议、存储与迁移
│  ├─ web/                # React 管理端产品应用
│  └─ wiki/               # UI 组件与业务模式预览、可访问性验收入口
├─ packages/              # 共享库
│  ├─ ui/                 # 共享 React 组件、布局、主题与设计令牌
│  ├─ devmock/            # 浏览器与 Node 双端 Mock、场景包和契约模拟
│  ├─ eslint-config/      # 前端共享 ESLint 严格配置
│  └─ typescript-config/  # 前端共享 TypeScript 严格配置
├─ docs/                  # PRD、ARCHITECTURE、API、ADR、协议规格与运维文档
├─ scripts/               # 质量门、容器化开发与验收入口
└─ 根工作区配置           # Cargo / pnpm / Turborepo 工作区编排文件
```

可构建子工程分两类根：`apps/`（可运行入口）与 `packages/`（共享库）；禁止再新增第三套子工程根目录（ADR-0016）。

## 开发方式

统一入口为根 `Makefile`（每个目标都是 Docker 的薄封装）：

```bash
make check     # 质量门：格式、静态分析、测试（与 CI 同脚本）
make release   # 发布构建：产出 Linux x86_64 tar 包到 dist/
make deploy    # 部署到 .env 配置的 Linux 主机（rootless 用户级 systemd）
make rollback  # 目标机回滚到上一发布
make clean     # 清空发布输出目录
```

无 make 时质量门也可直接用 `./scripts/check.sh`（Windows：`./scripts/check.ps1`），与 `make check` 完全同源。

硬约束：

- 宿主机禁止直接运行 `cargo`、`pnpm`、`npm` 等构建测试命令，全部经 Docker 执行。
- 宿主项目目录不得出现 `target/`、`node_modules/` 等构建产物（由 Docker 命名卷承接）。
- CI 与本地使用同一组脚本（`scripts/quality-gate.sh`）。
- 禁止创建 git worktree 作为并行开发或测试手段。

## 文档导航

- 需求：[`docs/PRD.md`](docs/PRD.md)
- 架构：[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md)
- 接口：[`docs/API.md`](docs/API.md)
- 决策：[`docs/adr/`](docs/adr/)
- 规格：[`docs/specs/`](docs/specs/)
- 运维：[`docs/OPERATIONS.md`](docs/OPERATIONS.md)
- 安全：[`SECURITY.md`](SECURITY.md)
- 演进与维护：[`docs/CONTRIBUTING.md`](docs/CONTRIBUTING.md)
- 变更史：[`CHANGELOG.md`](CHANGELOG.md)

## 许可

MIT，见 [LICENSE](LICENSE)。
