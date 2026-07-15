# JianArtifact 统一构建入口（M1-FR-042 / 043）
# 构建、测试、打包一律在 Docker 容器内执行，宿主机禁直接运行 cargo/pnpm；
# 部署/回滚在宿主经 SSH 执行（bash + ssh/scp），目标机全程用户级 systemd、无 root。
# recipe 全部写成单条命令，保证在 cmd 与 sh 两种宿主 shell 下行为一致。

.DEFAULT_GOAL := help
.PHONY: help check e2e e2e-real acceptance acceptance-curl acceptance-maven acceptance-npm performance release deploy rollback clean openapi

# 列出可用目标
help:
	@echo JianArtifact 构建入口
	@echo   make check     质量门：格式、静态分析、测试（与 CI 同脚本，容器内）
	@echo   make e2e       前端对 devmock 的真实浏览器 E2E（Playwright，容器内）
	@echo   make e2e-real  前端对真实 Rust 后端的浏览器 E2E（容器内）
	@echo   make acceptance 官方 curl、Maven、npm 客户端验收（容器内）
	@echo   make acceptance-curl 或 acceptance-maven 或 acceptance-npm 按协议域验收
	@echo   make performance 5 GiB cgroup 性能验收与证据（容器内）
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

# 前端真实浏览器 E2E（Playwright 驱动 vite dev + devmock）。
# make check 的第 9 步已包含 E2E；此目标供只跑 E2E 的快速迭代使用
e2e:
	docker compose run --rm dev bash -c "pnpm install --frozen-lockfile && pnpm --filter @jianartifact/web run test:e2e"

# 前端连接隔离的真实 Rust 后端执行 Playwright E2E
e2e-real:
	docker compose run --rm dev bash -c "pnpm install --frozen-lockfile && pnpm --filter @jianartifact/web run build && pnpm --filter @jianartifact/web run test:e2e:real"

# 官方客户端协议验收；各域可独立迭代，统一入口按 curl、Maven、npm 顺序执行
acceptance:
	docker compose run --rm dev bash scripts/acceptance/run.sh all

acceptance-curl:
	docker compose run --rm dev bash scripts/acceptance/run.sh curl

acceptance-maven:
	docker compose run --rm dev bash scripts/acceptance/run.sh maven

acceptance-npm:
	docker compose run --rm dev bash scripts/acceptance/run.sh npm

# 性能验收与客户端验收共用隔离服务、受控上游和临时运行目录
performance:
	docker compose run --rm dev bash -c "ACCEPTANCE_RELEASE_BINARY=true bash scripts/acceptance/run.sh performance"

# 部署：自动依赖 release，防止复用陈旧发布包
deploy: release
	bash scripts/deploy.sh deploy

# 回滚到目标机上一发布（不需要重新构建）
rollback:
	bash scripts/deploy.sh rollback

# 清理发布输出（同样在容器内执行，避免依赖宿主 shell 的删除命令）
clean:
	docker compose run --rm dev rm -rf dist
