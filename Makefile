# JianArtifact 统一构建入口（M1-FR-042）
# 所有目标均为 Docker 的薄封装：构建、测试、打包一律在容器内执行，宿主机禁直接运行 cargo/pnpm。
# recipe 全部写成单条 docker 命令，保证在 cmd 与 sh 两种宿主 shell 下行为一致。
# 部署与回滚目标（deploy / rollback）随 M1-FR-043 落地后补充。

.DEFAULT_GOAL := help
.PHONY: help check release clean

# 列出可用目标
help:
	@echo JianArtifact 构建入口（全部在 Docker 容器内执行）
	@echo   make check    质量门：格式、静态分析、测试（与 CI 同脚本）
	@echo   make release  发布构建：产出 Linux x86_64 tar 包到 dist/
	@echo   make clean    清空发布输出目录 dist/

# 质量门：与 scripts/check.sh 及 CI 完全同源
check:
	docker compose run --rm dev bash scripts/quality-gate.sh

# 发布构建与打包
release:
	docker compose run --rm dev bash scripts/build-release.sh

# 清理发布输出（同样在容器内执行，避免依赖宿主 shell 的删除命令）
clean:
	docker compose run --rm dev rm -rf dist
