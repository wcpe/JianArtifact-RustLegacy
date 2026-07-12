# JianArtifact 统一构建入口（M1-FR-042 / 043）
# 构建、测试、打包一律在 Docker 容器内执行，宿主机禁直接运行 cargo/pnpm；
# 部署/回滚在宿主经 SSH 执行（bash + ssh/scp），目标机全程用户级 systemd、无 root。
# recipe 全部写成单条命令，保证在 cmd 与 sh 两种宿主 shell 下行为一致。

.DEFAULT_GOAL := help
.PHONY: help check release deploy rollback clean openapi

# 列出可用目标
help:
	@echo JianArtifact 构建入口
	@echo   make check     质量门：格式、静态分析、测试（与 CI 同脚本，容器内）
	@echo   make openapi   重新生成管理 API 契约快照 packages/devmock/openapi.json（容器内）
	@echo   make release   发布构建：产出 Linux x86_64 tar 包到 dist/（容器内）
	@echo   make deploy    部署到 .env 配置的目标机（自动依赖 release，rootless）
	@echo   make rollback  目标机回滚到上一发布
	@echo   make clean     清空发布输出目录 dist/

# 重新生成契约快照与 TypeScript 类型（devmock 与前端的消费入口，质量门校验其新鲜度）
openapi:
	docker compose run --rm dev bash -c "cargo run -q --bin jianartifact -- openapi > packages/devmock/openapi.json && pnpm --filter @jianartifact/devmock run generate:schema"

# 质量门：与 scripts/check.sh 及 CI 完全同源
check:
	docker compose run --rm dev bash scripts/quality-gate.sh

# 发布构建与打包
release:
	docker compose run --rm dev bash scripts/build-release.sh

# 部署：自动依赖 release，防止复用陈旧发布包
deploy: release
	bash scripts/deploy.sh deploy

# 回滚到目标机上一发布（不需要重新构建）
rollback:
	bash scripts/deploy.sh rollback

# 清理发布输出（同样在容器内执行，避免依赖宿主 shell 的删除命令）
clean:
	docker compose run --rm dev rm -rf dist
