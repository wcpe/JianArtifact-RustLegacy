# 变更日志

本项目所有重要变更记录于此。

格式遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

## 未发布版本

### 新增
- 建立工程骨架：Cargo / pnpm / Turborepo 工作区、Docker 化统一质量门与 CI 工作流。
- Makefile 统一构建入口（check / release / clean）与容器内 Linux 发布构建：产出 x86_64 tar 发布包（含二进制、用户级 systemd unit 模板、VERSION 与 sha256 校验和），构建前强制校验 VERSION 与 Cargo 版本一致。
- rootless 部署与回滚（make deploy / rollback）：SSH 上传经 sha256 校验后落 releases 目录并以 current 符号链接原子切换，用户级 systemd 全程无 root；健康确认失败自动回滚上一发布，保留份数可配置；部署逻辑测试（桩 systemd）纳入质量门。

> 发版时把"未发布版本"段切成 `## [X.Y.Z] - YYYY-MM-DD`，再新建空的"未发布版本"段。
