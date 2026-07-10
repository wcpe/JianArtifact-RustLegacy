# 功能规格：代理测试重做（测表单当前填的代理）

> 状态：已交付@v0.7.0（自动化与真机验收通过）　·　关联 PRD：FR-147（增强 FR-128，依赖 FR-146）

## 1. 背景与目标

FR-128 的测试按钮测的是**已保存生效**的代理，与表单编辑态脱节：填完必须先保存才能验证，且旧实现把代理地址误预填为测试目标（必失败）；测试被中断后 loading 状态不复位导致按钮永久转圈。FR-147 让测试直接验证**表单当前填写的代理**，默认目标 `https://www.google.com`，并根治 loading 卡死。属 P2。

## 2. 需求（要什么）

- `POST /settings/proxy-test` 请求体新增可选 `proxy`（`{url, username?, password?}`，形同 PATCH 的 ProxyEntryPatch）：给出即用其**临时构造** client 测试；密码缺省时回落已存凭据（同用户名才沿用，复用 `rebuild_proxy_url` 三态）；`proxy.url` 空白 = 测直连。
- 缺省 `proxy` 字段时行为不变（测当前生效代理），兼容旧调用。
- 代理构造失败（非法 scheme 等）返 400 中文错误（不含凭据）。
- 前端：测试按钮把表单当前值作为 `proxy` 随请求发送（无需先保存）；模态框说明文案点明「测试使用表单中当前填写的代理」。
- 默认目标 URL：`https://www.google.com`（i18n 占位值），预填目标输入框。
- loading 永不卡死：中断（关模态框 / 重开）后 `testing` 状态复位；双层超时（前端 10s race + 后端 10s reqwest）保持。
- 不做（范围外）：测试结果持久化、批量测试、非 Admin 开放。

## 3. 设计（怎么做）

- 后端 `src/api/settings.rs::proxy_test`：`ProxyTestRequest` 增 `proxy: Option<ProxyEntryPatch>`；Some 时 `rebuild_proxy_url(patch, 当前生效存储 URL)` → `build_outbound_client(10s, &临时配置)` 得临时 client（失败 400）；None 走 `state.settings.network.client()`。目标 URL 校验与响应结构不变。
- 前端 `endpoints.ts::testProxy` 增可选 `proxy` 参数；`SettingsPage.tsx` 传表单值、修 `openTestModal` 复位 `testing`；i18n `testUrlPlaceholder` 改 `https://www.google.com`。

## 4. 任务拆分

- [x] 后端：请求体扩展 + 临时 client 构造 + 测试（非法 scheme 400 / 不可达 ok:false / 经本地 mock 代理 ok:true 证明真用了表单代理 / 缺省 proxy 行为不变）
- [x] 前端：endpoints + SettingsPage 传表单代理 + loading 复位 + 占位 google + 组件测试
- [x] 前端 F-01：代理 URL 字段清除后重新填写，测试请求必须发送重填后的当前值，不得回落清除前的旧值
- [x] 后端：请求显式携带表单代理时，不得继承已保存 `no_proxy` 而绕过该代理
- [x] 测试补齐：验证真实 abort、HTTP wire-contract，以及目标 URL 格式非法时返回 400
- [x] 文档：API.md proxy-test 请求体、CHANGELOG、PRD 状态

## 5. 验收标准

- 后端测试绿：上述四类用例全过；错误信息不含凭据。
- 前端测试绿：请求体含表单代理三字段（密码三态）、占位 google、中断后重开不卡 loading。
- F-01 已覆盖：代理 URL 清除后重填，断言请求只携带重填后的当前值。
- 显式表单代理不受已保存 `no_proxy` 绕过；真实 abort、HTTP wire-contract、目标 URL 格式非法返回 400 均有自动化验证。
- **真机（已通过）**：设置页分别填写 HTTP 与 SOCKS5 代理且不保存即可测试成功；不可达目标正确显示失败；测试中关闭/重开、编辑/重置目标均会复位 loading 并抑制旧结果，刷新后表单代理未持久化。

## 6. 风险 / 待定

- 测试端点用用户提供的代理 URL 出站（仅 Admin、显式触发、目标仍限 http/https，SSRF 面不变）。
