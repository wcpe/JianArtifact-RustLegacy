# 变更日志

本项目所有重要变更记录于此。

格式遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

## 未发布版本

### 新增
- 建立工程骨架：Cargo / pnpm / Turborepo 工作区、Docker 化统一质量门与 CI 工作流。
- Makefile 统一构建入口（check / release / clean）与容器内 Linux 发布构建：产出 x86_64 tar 发布包（含二进制、用户级 systemd unit 模板、VERSION 与 sha256 校验和），构建前强制校验 VERSION 与 Cargo 版本一致。
- rootless 部署与回滚（make deploy / rollback）：SSH 上传经 sha256 校验后落 releases 目录并以 current 符号链接原子切换，用户级 systemd 全程无 root；健康确认失败自动回滚上一发布，保留份数可配置；部署逻辑测试（桩 systemd）纳入质量门。
- 前端工程基座（React 19 + Tailwind 4 + Vite 7）：web / ui / wiki / devmock 四工程脚手架与共享严格配置（TS strict + noUncheckedIndexedAccess、ESLint 9 严格集 --max-warnings 0、Prettier 只检不改）接入 Docker 质量门；ui 经 shadcn CLI 收录 Button 组件源码，wiki 提供组件预览矩阵，devmock 建立 MSW 双端骨架。
- 工作区结构调整为 apps/（可运行入口）+ packages/（共享库）双根（ADR-0016）。
- 后端服务基座与认证域契约骨架：axum 服务（配置加载/中文日志/SQLite 迁移框架/统一错误结构/请求标识）、健康检查与就绪自检（/healthz、/readyz 真实现）、登录/刷新/退出/API Token/用户管理契约端点（utoipa OpenAPI 唯一真源，GET /api/v1/openapi.json 与 `jianartifact openapi` 导出）；cargo-audit 与 cargo-deny 许可证检查接入质量门。
- 仓库域与迁移域契约骨架：仓库 CRUD（删除双模式 + 确认头）、ACL、浏览/搜索/使用片段；迁移三类来源、发现、计划与冲突、十态任务生命周期、分仓报告——管理 API 六域契约面齐备（32 个 operation）；契约快照 `packages/devmock/openapi.json` 新鲜度校验与 devmock 无幻影端点比对测试纳入质量门（make openapi 重新生成）。
- devmock 七类场景包与有状态内存模型（M1-FR-035）：认证/Token/用户/仓库/浏览/迁移全语义、契约双向硬断言（无幻影 + 全量覆盖）；openapi-typescript 契约类型生成接线。
- 共享组件库四层（M1-FR-036）：基础（shadcn 源码 12 组件）、复合（StatusBadge/Pagination/DataTable 基于 TanStack Table）、状态（Empty/Error/Loading/Forbidden）、布局（AppShell/PageHeader）；wiki 重写为组件 × 状态矩阵验收站。
- 管理端六组页面对 devmock 开发（M1-FR-034 向导 UI 等）：登录/仪表盘/仓库列表与详情（文件浏览+使用方式）/API Token（明文一次性展示）/用户管理/迁移向导（源配置→发现→计划→执行→进度轮询→报告，task_id 落 localStorage 刷新可找回）；三层守卫路由 + 会话上下文 + 契约类型 API client；mock 开关经构建期常量守卫实现生产产物零 mock 代码零测试凭据（产物 grep 为证）。

> 发版时把"未发布版本"段切成 `## [X.Y.Z] - YYYY-MM-DD`，再新建空的"未发布版本"段。
