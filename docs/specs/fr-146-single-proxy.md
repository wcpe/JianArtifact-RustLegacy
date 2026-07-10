# 功能规格：单一通用出站代理

> 状态：已交付@v0.7.0（自动化与真机验收通过）　·　关联 PRD：FR-146　·　取代决策见 ADR-0042

## 1. 背景与目标

真机运维反馈：`http` / `https` / `all` 三个代理槽位心智负担高、易配错（三个都填、填错槽位），实际部署几乎总是"一个代理全局走"。FR-146 把出站代理收敛为**单一通用代理**：一个 URL（用户自填 `http://` / `https://` / `socks5://` / `socks5h://` 前缀）+ 一份凭据，经 `reqwest::Proxy::all` 注入全 scheme。属 P2。

## 2. 需求（要什么）

- `[network.proxy]` 收敛为单键 `url`（env `JIANARTIFACT_NETWORK_PROXY_URL`）+ 既有 `no_proxy`。
- URL scheme 仅接受 `http://` / `https://` / `socks5://` / `socks5h://`，其余给中文校验错误。
- 旧键 `http` / `https` / `all`（TOML 与 env）**自动映射**为 `url`（优先级 `all > https > http`）并启动 WARN 废弃提示；`url` 与旧键同时给值时 `url` 为准、WARN 旧键被忽略。
- 凭据保持 **URL + 用户名 + 密码三字段**（密码三态不回显，沿用 ADR-0024 安全模型与 ADR-0030 加密落库）。
- 设置页只剩一个代理槽位；测试按钮行为本 FR 不动（FR-147 重做）。
- 旧加密落库形态（`http`/`https`/`all` 三项 JSON）启动恢复时按同一优先级折叠，不丢已存凭据。
- 不做（范围外）：按 host 多代理路由、SOCKS4、代理测试语义变更（FR-147）、PATCH 契约对旧三键的兼容（前端与后端同二进制发布，无旧新组合；外部脚本变更记入 CHANGELOG）。

## 3. 设计（怎么做）

决策正文见 [ADR-0042](../adr/0042-single-unified-outbound-proxy.md)（取代 ADR-0024 的"三键"决策；共享 client、热替换、密码三态、加密落库等沿用）。

- **config**（`src/config.rs`）：
  - `NetworkProxyConfig` 改为 `{ url, no_proxy }` + 三个仅反序列化用的旧键字段（`http`/`https`/`all`，标注废弃）；
  - 新增 `normalize()`：折叠旧键 → `url`（`all > https > http`），返回 WARN 文案列表由调用方打日志（`config` 层不 tracing 的现状若已有 tracing 则直接打）；装载入口调用；
  - `build_outbound_client`：仅注入 `Proxy::all(url)`（含 scheme 校验、错误不含凭据）；
  - `NetworkState` / 快照结构随之简化（字段类型不变，仍为 `NetworkProxyConfig`）。
- **api/settings**（`src/api/settings.rs`）：
  - 视图 `NetworkProxyView` → `{ proxy: ProxyEntryView, no_proxy }`；
  - PATCH `NetworkProxyPatch` → `{ proxy: ProxyEntryPatch, no_proxy }`；`rebuild_proxy_url` 复用不变；
  - 落库 `PersistedProxy` 新增 `proxy` 单项；旧 `http`/`https`/`all` 字段保留为只读兼容（恢复时折叠，优先 `proxy` > `all` > `https` > `http`）；新写入只写 `proxy` + `no_proxy`。
- **前端**：`types.ts` / `endpoints.ts` / `SettingsPage.tsx`（三槽位 → 单槽位"通用网络代理"，占位提示 `socks5://host:1080 或 http://host:3128`，附 socks5h 远端 DNS 提示）/ mock store / i18n / 组件测试。
- **实机测试**：`tests/real_proxy_outbound.rs` 随新配置结构更新。

## 4. 任务拆分

- [x] ADR-0042 + 标记 ADR-0024 部分被取代
- [x] 后端：config 单键 + 旧键折叠 + client 构造 + 测试
- [x] 后端：settings 视图 / PATCH / 加密落库单项化 + 旧落库兼容 + 测试
- [x] 前端：单槽位界面 + types + mock + i18n + 测试
- [x] 文档同步：CONFIG、API、ARCHITECTURE、CHANGELOG、config.example.toml、PRD 状态

## 5. 验收标准

- 单元 / 集成测试绿：旧键 TOML / env 折叠（三种优先级组合 + 同给 WARN）、scheme 校验拒绝非法前缀、HTTP absolute-form、HTTPS 经 HTTP 代理 CONNECT、SOCKS5H 域名握手、旧加密落库 JSON 恢复折叠正确且凭据不丢。
- 前端 lint + vitest 绿：单槽位渲染、保存 PATCH 契约、密码三态交互。
- 本地生产调用链验证：proxy 回源与漏洞库下载均从共享代理槽取 client；真实 GitHub 更新链由 `tests/real_proxy_outbound.rs` 提供 env 门控验收。
- **实机（已通过）**：服务器配置单一 socks5 与 http 代理各一次，proxy 回源 / 漏洞库刷新 / 更新检查 / 资产下载均经代理成功；本机 `tests/real_proxy_outbound.rs`（env 门控）预验两类代理各 2/2 通过。
- 旧配置升级：携 `http`/`https`/`all` 旧键启动，服务可用且日志出现废弃 WARN。

## 6. 风险 / 待定

- PATCH / GET 契约为破坏性变更（外部脚本），记 CHANGELOG 提示；前端同二进制无风险。
- 旧落库 JSON 的折叠只发生在读取时，首次 PATCH 后即写新形态（自然迁移，无需 SQL migration）。
