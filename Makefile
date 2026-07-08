# JianArtifact 本地开发 / 构建命令集（开发便利，非 CI 真源；CI 见 .github/workflows/ci.yml）。
#
# 约定：
# - 后端 cargo 一律经 rustup run 1.96.0 跑（匹配 CI 浮动 @stable=1.96，本地默认 1.94.1 编不过）。
# - 构建顺序硬约束：必须先建前端（产出 frontend/dist）再编后端——rust-embed 编译期读 dist，
#   否则后端嵌入的是占位空集、与发布产物不一致。
# - 前端经 pnpm（lockfileVersion 9 / pnpm 9）。
#
# 可覆盖变量：make CARGO="cargo" test  （如本机默认工具链已达标）

CARGO  ?= rustup run 1.96.0 cargo
PNPM   ?= pnpm -C frontend
PYTHON ?= python

ifneq (,$(wildcard .env))
include .env
export
endif

DEPLOY_ARCH ?= amd64
DEPLOY_PORT ?= 22
DEPLOY_USER ?= root
SSH_KEY_PATH ?= $(HOME)/.ssh/jianartifact_deploy_ed25519
CROSS_IMAGE ?= jianartifact-cross:local
BUILD_VERSION ?= 0.5.0
SERVICE_NAME ?= jianartifact
SERVICE_USER ?= jianartifact
INSTALL_DIR ?= /opt/jianartifact
CONFIG_DIR ?= /etc/jianartifact
DATA_DIR ?= /var/lib/jianartifact
LISTEN_ADDR ?= 0.0.0.0
PORT ?= 9999

ifeq ($(DEPLOY_ARCH),amd64)
RUST_TARGET := x86_64-unknown-linux-musl
LINUX_ARCH := x86_64
else ifeq ($(DEPLOY_ARCH),arm64)
RUST_TARGET := aarch64-unknown-linux-musl
LINUX_ARCH := aarch64
else
RUST_TARGET := invalid-target
LINUX_ARCH := invalid-arch
endif

ARTIFACT_TARBALL_RESOLVED := $(if $(ARTIFACT_TARBALL),$(ARTIFACT_TARBALL),output/linux/jianartifact-$(BUILD_VERSION)-linux-$(LINUX_ARCH).tar.gz)
BINARY_NAME_RESOLVED := $(if $(BINARY_NAME),$(BINARY_NAME),jianartifact-linux-$(LINUX_ARCH))

.DEFAULT_GOAL := help

.PHONY: help install web-dev run build build-web release \
        test test-rs test-web fmt fmt-check lint check audit clean \
        env ssh-key validate-deploy-arch validate-deploy-env cross-image cross-build deploy

help: ## 列出所有可用命令
	@echo "JianArtifact 开发命令："
	@echo "  make install     安装前端依赖（pnpm install）"
	@echo "  make web-dev     启动前端开发服务器（vite，热重载，仅 UI）"
	@echo "  make run         建前端 + cargo run，跑完整单二进制（含嵌入 UI）"
	@echo "  make build       建前端 + 后端 debug 构建（产出可运行二进制）"
	@echo "  make build-web   仅构建前端（产出 frontend/dist）"
	@echo "  make release     建前端 + 后端 release 构建（strip+LTO，发布用）"
	@echo "  make test        全量测试（后端 cargo test + 前端 vitest）"
	@echo "  make test-rs     仅后端测试"
	@echo "  make test-web    仅前端测试（vitest）"
	@echo "  make fmt         格式化（cargo fmt + prettier --write）"
	@echo "  make fmt-check   格式检查（cargo fmt --check + eslint/prettier check）"
	@echo "  make lint        静态检查（cargo clippy -D warnings + 前端 lint）"
	@echo "  make check       本地提交前全门：建前端→fmt-check→clippy→cargo test→vitest（对齐 CI）"
	@echo "  make audit       依赖漏洞审计（cargo audit + pnpm audit --prod）"
	@echo "  make env         创建本地部署 .env（已存在则不覆盖）"
	@echo "  make ssh-key     生成本地部署 SSH key（已存在则不覆盖）"
	@echo "  make cross-image 构建 Docker 交叉编译镜像"
	@echo "  make cross-build Docker 内交叉编译 Linux amd64/arm64 包"
	@echo "  make deploy      交叉编译并部署到 Linux systemd 服务器"
	@echo "  make clean       清理 cargo target 与前端 dist 产物"

install: ## 安装前端依赖
	$(PNPM) install

web-dev: ## 前端开发服务器（vite，仅 UI 热重载）
	$(PNPM) dev

build-web: ## 仅构建前端（产出 frontend/dist；构建会清掉 .gitkeep，构建后恢复）
	$(PNPM) build
	-@git checkout -- frontend/dist/.gitkeep

run: build-web ## 建前端后跑完整二进制（含嵌入 UI）
	$(CARGO) run

build: build-web ## 建前端 + 后端 debug 构建
	$(CARGO) build

release: build-web ## 建前端 + 后端 release 构建（strip+LTO）
	$(CARGO) build --release

test-rs: ## 仅后端测试
	$(CARGO) test

test-web: ## 仅前端测试（vitest）
	$(PNPM) test

test: test-rs test-web ## 全量测试（后端 + 前端）

fmt: ## 格式化后端与前端
	$(CARGO) fmt
	$(PNPM) format

fmt-check: ## 格式检查（不改文件）
	$(CARGO) fmt --check
	$(PNPM) lint

lint: ## 静态检查（clippy 告警即错 + 前端 lint）
	$(CARGO) clippy --all-targets -- -D warnings
	$(PNPM) lint

# 本地提交前全门：严格对齐 CI（ci.yml）——先建前端供 rust-embed，再后端 fmt/clippy/test，最后前端测试。
check: build-web ## 提交前全门（对齐 CI）
	$(CARGO) fmt --check
	$(CARGO) clippy --all-targets -- -D warnings
	$(CARGO) test
	$(PNPM) test

audit: ## 依赖漏洞审计
	$(CARGO) audit
	$(PNPM) audit --prod

env: ## 创建本地部署 .env（已存在则不覆盖）
	@if [ -f .env ]; then \
		printf '已存在 .env，保持不覆盖。\n'; \
	else \
		$(PYTHON) -c "from pathlib import Path; lines=['# JianArtifact Linux systemd \u90e8\u7f72\u53d8\u91cf\uff08\u672c\u5730\u6587\u4ef6\uff0c\u4e0d\u5165\u5e93\uff09','# \u8bf7\u5148\u8fd0\u884c make ssh-key\uff0c\u628a ~/.ssh/jianartifact_deploy_ed25519.pub \u5185\u5bb9\u52a0\u5165\u670d\u52a1\u5668 authorized_keys\u3002','','# SSH \u8fde\u63a5','DEPLOY_HOST=','DEPLOY_PORT=22','DEPLOY_USER=root','','# \u6784\u5efa\u4e0e\u76ee\u6807\u67b6\u6784\uff1aamd64 \u6216 arm64','DEPLOY_ARCH=amd64','CROSS_IMAGE=jianartifact-cross:local','BUILD_VERSION=0.5.0','','# \u672c\u5730\u4ea7\u7269\uff1b\u7559\u7a7a\u65f6\u7531 make \u6309 DEPLOY_ARCH \u81ea\u52a8\u63a8\u5bfc','ARTIFACT_TARBALL=','BINARY_NAME=','','# \u8fdc\u7aef\u5e03\u5c40','SERVICE_NAME=jianartifact','SERVICE_USER=jianartifact','INSTALL_DIR=/opt/jianartifact','CONFIG_DIR=/etc/jianartifact','DATA_DIR=/var/lib/jianartifact','','# \u5e94\u7528\u8fd0\u884c','LISTEN_ADDR=0.0.0.0','PORT=9999','PUBLIC_BASE_URL=','','# \u9996\u542f\u7ba1\u7406\u5458\u5f15\u5bfc\uff08\u7a7a\u5e93\u9996\u6b21\u542f\u52a8\u624d\u4f7f\u7528\uff0c\u586b\u5199\u540e\u8bf7\u59a5\u5584\u4fdd\u7ba1\u672c\u6587\u4ef6\uff09','JIANARTIFACT_ADMIN_USERNAME=','JIANARTIFACT_ADMIN_PASSWORD=']; Path('.env').write_text('\n'.join(lines) + '\n', encoding='utf-8')"; \
		printf '已创建 .env，请填写 DEPLOY_HOST 等部署变量。\n'; \
	fi

ssh-key: ## 生成本地部署 SSH key（已存在则不覆盖）
	@mkdir -p "$$(dirname "$(SSH_KEY_PATH)")"
	@if [ -f "$(SSH_KEY_PATH)" ]; then \
		printf '已存在 SSH 私钥 %s，保持不覆盖。\n' "$(SSH_KEY_PATH)"; \
	else \
		ssh-keygen -t ed25519 -f "$(SSH_KEY_PATH)" -N "" -C "jianartifact-deploy"; \
		printf '已生成部署 SSH key：%s\n' "$(SSH_KEY_PATH)"; \
	fi
	@printf '请把以下公钥加入服务器用户的 ~/.ssh/authorized_keys：\n'
	@cat "$(SSH_KEY_PATH).pub"

validate-deploy-arch: ## 校验部署架构参数
	@if [ "$(DEPLOY_ARCH)" != "amd64" ] && [ "$(DEPLOY_ARCH)" != "arm64" ]; then \
		printf 'DEPLOY_ARCH 只能是 amd64 或 arm64，当前为：%s\n' "$(DEPLOY_ARCH)"; \
		exit 2; \
	fi

validate-deploy-env: validate-deploy-arch ## 校验部署必填变量
	@if [ -z "$(DEPLOY_HOST)" ]; then printf '请先在 .env 填写 DEPLOY_HOST。\n'; exit 2; fi
	@if [ -z "$(DEPLOY_USER)" ]; then printf '请先在 .env 填写 DEPLOY_USER。\n'; exit 2; fi
	@if [ ! -f "$(SSH_KEY_PATH)" ]; then printf '未找到 SSH 私钥：%s，请先运行 make ssh-key。\n' "$(SSH_KEY_PATH)"; exit 2; fi

cross-image: ## 构建 Docker 交叉编译镜像
	docker build -f Dockerfile.cross -t "$(CROSS_IMAGE)" .

cross-build: validate-deploy-arch cross-image ## Docker 内交叉编译 Linux 包
	mkdir -p output/linux
	MSYS_NO_PATHCONV=1 docker run --rm -v "$(CURDIR):/workspace" -w /workspace "$(CROSS_IMAGE)" bash -lc 'set -euo pipefail; pnpm -C frontend install --frozen-lockfile; pnpm -C frontend build; cargo zigbuild --release --target $(RUST_TARGET); install -m 0755 target/$(RUST_TARGET)/release/jianartifact output/linux/$(BINARY_NAME_RESOLVED); tar -C output/linux -czf $(ARTIFACT_TARBALL_RESOLVED) $(BINARY_NAME_RESOLVED); printf "已产出 %s\\n" "$(ARTIFACT_TARBALL_RESOLVED)"'

deploy: env ssh-key validate-deploy-env ## 交叉编译并部署到 Linux systemd 服务器
	@if [ -z "$(ARTIFACT_TARBALL)" ]; then \
		"$(MAKE)" cross-build; \
	else \
		printf '使用已配置部署包：%s\n' "$(ARTIFACT_TARBALL)"; \
	fi
	ARTIFACT_TARBALL="$(ARTIFACT_TARBALL_RESOLVED)" BINARY_NAME="$(BINARY_NAME_RESOLVED)" bash scripts/deploy-linux-systemd.sh

clean: ## 清理构建产物
	$(CARGO) clean
	rm -rf frontend/dist/assets frontend/dist/index.html
	-@git checkout -- frontend/dist/.gitkeep
