# 唯一合法的本地质量门入口（Windows / PowerShell）：
# 一切构建、测试、检查均在 Docker 容器内执行，宿主机禁止直接运行 cargo/pnpm/npm。
$ErrorActionPreference = "Stop"

# 切到仓库根目录，保证 docker compose 使用项目根的编排文件
Set-Location (Join-Path $PSScriptRoot "..")

docker compose run --rm dev bash scripts/quality-gate.sh
exit $LASTEXITCODE
