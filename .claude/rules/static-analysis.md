# 代码风格与静态检查（防风格 / 质量漂移）

> 统一各组件的格式化与静态检查工具，并要求 CI 强制门禁——风格一致、低级问题挡在合并前。

## 1. 各组件工具链

- **Rust 后端（`apps/server`）**：`rustfmt --check` 格式检查 + `clippy --all-targets --all-features -D warnings`（警告即失败）+ `cargo-audit`（依赖漏洞）+ 许可证机器检查（后两项随首个 Rust 依赖引入时接入质量门脚本，见 `docs/ROADMAP.md` 阶段 0）。
- **前端（`apps/web` / `apps/ui` / `apps/wiki` / `apps/devmock`）**：`prettier --check`（只检查不改写）+ `eslint --max-warnings 0`（共享配置来自 `apps/eslint-config`）+ `tsc` typecheck（`strict` + `noUncheckedIndexedAccess` + `isolatedModules`，共享配置来自 `apps/typescript-config`）；随前端工程基座（M1-FR-044）落地接线。

以上检查**全部经 Docker 质量门入口 `scripts/check.sh`（或 `check.ps1`）执行，宿主机禁止直接运行 `cargo` / `pnpm`**。配置文件放在对应组件工程内或仓库根，版本固定。

## 2. 强制要求

- **CI 门禁**：lint 与格式检查未过 → 不允许合并（与测试同级，见 `testing-and-quality.md`）。CI（`.github/workflows/ci.yml`）与本地 Docker 质量门使用同一组脚本，避免"本地通过、CI 不同命令失败"。
- **本地**：提交前经 `scripts/check.sh`（或 `check.ps1`）跑对应组件的 format + lint，不把格式问题留给 CI。
- **依赖漏洞与许可证**：Rust 用 `cargo-audit` 发现依赖漏洞，并配许可证机器检查，均纳入同一质量门（接线时机见 §1）。
- 工具与规则版本固定（写进配置 / 构建），避免不同机器结果不一致。

## 3. 与现有规则的关系

- 本规则是 `testing-and-quality.md` 的补充：测试管"行为对不对"，静态检查管"写法干不干净"。
- 禁用某条 lint 要在**配置里集中声明并注明原因**，不在代码里零散 `#[allow(...)]` / `// eslint-disable` 关闭（除非有明确理由并写明）。
