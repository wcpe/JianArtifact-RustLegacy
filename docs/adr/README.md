# 架构决策记录（ADR）

记录本项目的重大架构决策：背景、决策、理由、后果与被否的备选。每条决策一页，便于后来者理解"为什么是这样"。

| 编号 | 决策 | 状态 |
|---|---|---|
| [0001](0001-tech-stack-and-workspace.md) | 技术栈选型与单一工作区布局 | 已接受 |
| [0002](0002-sqlite-metadata-and-blob-storage.md) | SQLite 元数据唯一真源与文件系统 blob 存储 | 已接受 |
| [0003](0003-authentication-and-session.md) | 认证与会话方案 | 已接受 |
| [0004](0004-authorization-model.md) | 授权模型 | 已接受 |
| [0005](0005-repository-types.md) | 仓库类型模型（hosted/proxy/group） | 已接受 |
| [0006](0006-protocol-adapter-boundary.md) | 格式协议适配层边界 | 已接受 |
| [0007](0007-nexus-migration-strategy.md) | Nexus 迁移策略 | 已接受 |
| [0008](0008-offline-migration-package.md) | 离线迁移包格式 | 已接受 |
| [0009](0009-migration-task-persistence.md) | 迁移任务持久化与状态机 | 已接受 |
| [0010](0010-quality-gates-and-docker.md) | 质量门禁与 Docker 化验证 | 已接受 |
| [0011](0011-ui-design-system.md) | UI 设计系统 | 已接受 |
| [0012](0012-api-contract-source.md) | API 契约唯一真源 | 已接受 |
| [0013](0013-single-binary-delivery.md) | 单二进制交付 | 已接受 |
| [0014](0014-frontend-first-delivery.md) | 前端先行交付工序与契约骨架 | 已接受 |

> 模板：[`_template.md`](_template.md)（状态 / 背景 / 决策 / 理由 / 后果 / 备选方案），新 ADR 从模板复制起稿、顺延编号。

> **别慌通读**：ADR 有意稀少（只为重大决策写），理解现状看 [`../ARCHITECTURE.md`](../ARCHITECTURE.md)，ADR 只按需查"为什么"；被取代的归档不打扰，当前架构 = 未取代的活跃集。增长过快是滥写信号——日常变更归 PRD 状态列 + CHANGELOG。
