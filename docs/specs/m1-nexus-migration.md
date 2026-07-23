# 功能规格：Nexus 仓库全量迁移（M1-FR-022~034、037~040）

> 状态：阶段 4 Wave 5 外部验收阻塞　·　关联 PRD：M1-FR-022~034、037、038、039、040　·　架构决策：ADR-0007、ADR-0009、ADR-0015、ADR-0017

## 1. 背景、范围与施工原则

本规格定义从 Nexus Repository OSS 3.70.x 迁移 Raw、Maven、npm 的 hosted、proxy、group 仓库到 JianArtifact 的完整施工契约。首选来源是离线直读 Nexus 文件型 blob store；在线 REST 是数据不在本机时的备选；JianArtifact 自有离线包是可选的可移植归档形态。迁移落库必须经过对应格式的领域服务，禁止绕过协议语义直接写索引。

范围只含仓库配置、成员关系、受支持对象与可枚举正缓存；不迁 Nexus 用户、角色、权限、脚本、计划任务和系统配置。正式支持矩阵固定为 Nexus OSS 3.70.x + 文件型 blob store + Raw/Maven/npm，矩阵外来源在发现阶段阻断，不进入正式迁移任务。

阶段 4 在同一分支按 Wave 0~5 顺序推进，不创建 worktree。每一波完成后必须执行 ADR-0010 定义的完整严格质量门，门禁全绿才进入下一波；最终验收使用本地 Docker Nexus OSS 3.70.x，阶段 4 在真机矩阵完成前不得标为完成。

## 2. 来源、发现与计划快照

### 2.1 来源契约

- `native-dir`：首选。读取已停止 Nexus 或文件系统一致性快照中的 `.properties`/`.bytes`；仓库配置通过一次性 REST 拉取或由管理员手动补齐。运行中的活动数据目录不是合法全量来源。
- `online`：通过 Nexus REST 分页枚举配置、组件与资产并流式下载对象。创建发现、迁移和最终扫描时使用的 Nexus 用户名/口令只存在于当前进程内存。
- `package`：读取 `m1-nexus-offline-package.md` 定义的 v1 目录树并先完成整体校验。
- 裸 blob store：只恢复能明确归属的对象；proxy 上游或 group 成员缺失时标记配置不完整，最终状态不得为 `succeeded`。

离线路径必须位于配置的 `migration-workdir` 规范化根内。拒绝 `..`、绝对路径逃逸、软链接越界、空段、编码绕过和根外路径；服务端不提供目录浏览接口，也不泄露根外路径是否存在。

### 2.2 计划与不可变快照

计划确认后生成不可变参数快照，至少包含：来源种类及稳定源标识、源快照时间、仓库选择、源到目标映射、手动补充配置、proxy 凭据引用名、对象级冲突决策、空间估算基线和不支持项。明文 Nexus 用户名/口令不属于快照字段。

执行器不得原地修改快照。目标仓库或目标对象在计划确认后发生变化，导致已记录前置条件不再成立时，当前任务以稳定错误类别阻断并在报告中列出；管理员必须基于新发现结果创建新计划，不能把新决策补写进旧任务。

### 2.3 对象级异摘要冲突

目标路径不存在时可迁入；目标路径存在且 sha256 相同只计为 `skipped-identical`；目标路径存在且 sha256 不同不得覆盖，只允许以下三种显式决策：

- `rename`：把源仓库映射到新的目标仓库名；新目标仓库必须不存在。
- `map-empty`：把该源对象映射到明确指定、校验时为空的目标路径；执行前再次确认该路径仍为空。
- `abandon`：不迁移该对象；对象保留在报告中并使任务最终至少为 `partial`。

每条异摘要冲突决策必须在不可变快照和最终报告中同时记录：源仓库、源对象键、源 sha256、原目标仓库与路径、观察到的目标 sha256、决策枚举、决策后的目标仓库与路径。缺少决策、使用其他决策值或前置条件变化均阻断执行。

## 3. 任务状态、业务阶段与控制 API

### 3.1 十态保持不变

`TaskStatus` 固定复用 ADR-0009 的十态：

`queued / running / pausing / paused / cancelling / cancelled / succeeded / partial / failed / interrupted`

不得新增第十一状态。`awaiting-cutover` 是在线迁移的业务阶段，不是新的生命周期状态。

`TaskInfo` 新增必填字段 `phase`，枚举固定为：

- `initial-transfer`：首轮对象搬运，适用于全部来源。
- `awaiting-cutover`：仅在线来源；首轮搬运完成，等待管理员停止源端写入并显式发起最终切换。
- `finalizing`：仅在线来源；执行最终增量扫描、增量迁移和零漂移复核。
- `completed`：任务已经进入 `cancelled / succeeded / partial / failed` 之一。

状态与阶段的关键组合固定如下：

| 场景 | status | phase |
|---|---|---|
| 首轮执行 | `queued/running/pausing/paused/cancelling/interrupted` | `initial-transfer` |
| 在线首轮完成、等待切换 | `paused/interrupted` | `awaiting-cutover` |
| 在线最终扫描 | `running/pausing/paused/cancelling/interrupted` | `finalizing` |
| 终态 | `cancelled/succeeded/partial/failed` | `completed` |

普通人工暂停产生 `paused + initial-transfer/finalizing`，可调用 `resume`。`paused + awaiting-cutover` 是切换门，`resume` 必须返回 409，只有 `finalize` 能越过该门。

### 3.2 显式 finalize

新增：

`POST /api/v1/migration/tasks/{id}/finalize`

请求体可省略；若当前进程已丢失在线凭据，可与 `retry` 相同提交成对的 `username/password`，本次传入值与任务现有内存凭据合并判定。成功受理返回 202 与更新后的 `TaskInfo`。仅 `online` 来源且当前组合为 `paused + awaiting-cutover` 时合法；其他来源、其他状态或其他阶段返回 409。

固定流程：

1. 首轮搬运和首轮校验完成后，任务进入 `paused + awaiting-cutover`，报告给出停写要求，但服务不替管理员修改 Nexus 写入模式。
2. 管理员确认源端已经停写/只读后调用 `finalize`。
3. 任务进入 `running + finalizing`，扫描首轮快照后的新增、修改、删除；新增和修改按对象级幂等规则迁移，删除只记录到切换报告，不自动删除目标对象。
4. 增量处理后立即执行第二次验证扫描。只有零未处理漂移、零未解决配置项、零显式放弃和全部受支持对象双校验通过，才可进入 `succeeded + completed`。
5. 验证扫描发现源仍在变化时，不得成功；任务回到 `paused + awaiting-cutover`，记录新漂移并要求管理员重新确认停写后再次调用 `finalize`。

### 3.3 retry 与临时 Nexus 凭据

`POST /api/v1/migration/tasks/{id}/retry` 接受可选 JSON 请求体：

```json
{
  "username": "migration-user",
  "password": "temporary-secret"
}
```

约束固定如下：

- `username` 与 `password` 必须同时提供或同时省略；只提供一个字段返回 400。
- 离线且不需 REST 的任务使用空对象或无请求体。
- 在线任务只要后续步骤仍需访问 Nexus，重试时必须重新提交这对临时凭据；缺少时返回 409 和稳定错误码 `MIGRATION_CREDENTIALS_REQUIRED`。
- 凭据只进入当前进程的任务运行时内存槽，不写入 SQLite、不可变快照、对象检查点、日志、tracing 字段、错误正文、API 响应或报告；任务进入终态、取消或凭据被替换时立即丢弃。进程重启后该内存槽必然为空。
- 服务重启时，ADR-0009 定义的在途任务进入 `interrupted`。在线任务只要尚未终结且后续仍需回源，原状态即使是普通 `paused` 或 `paused + awaiting-cutover`，也因临时凭据已丢失而进入 `interrupted`，但保留原 `phase` 与检查点。
- 对原阶段为 `awaiting-cutover` 的中断任务，携带凭据的 `retry` 恢复为 `paused + awaiting-cutover`，随后由管理员调用 `finalize`；对 `initial-transfer/finalizing` 的中断任务，`retry` 恢复为 `running` 并从对象检查点续传。

`retry` 允许从 `failed / partial / interrupted / cancelled` 发起；终态任务只能恢复状态机内部保存的原执行阶段，调用方不能指定或跳过阶段。重试仍只处理失败、中断或内容变化对象；已一致对象精确计为跳过，不产生重复索引或孤儿 blob。

## 4. 执行、持久化与报告

- 对象提交顺序固定为「临时文件 → 流式写入并计算 sha256 → 校验 → 内容寻址落定 → 协议领域服务提交索引 → 写对象检查点」。暂停和取消只在该原子边界生效。
- hosted 必须通过源目标字节 sha256 与协议元数据不变量双校验；proxy 的每个可枚举正缓存必须迁移、校验或明确列为未迁移；group 必须保持成员顺序，成员缺失时不得产出半成品 group。
- 任务、不可变快照、`status`、`phase`、节流进度、对象检查点、分仓报告和有界失败样本持久化到 SQLite。临时凭据不得持久化。
- `succeeded` 严格要求零显式放弃、零未迁移受支持对象、零未解决配置项；任一仓库排除、缓存不可迁移、凭据引用缺失、裸 blob 降级或配置不完整均为 `partial`，并逐项计数。
- 报告按任务和仓库给出总数、完成数、迁移数、同摘要跳过数、失败数、放弃数、总字节、完成字节、耗时、失败类别、省略样本数、对象级冲突决策和在线切换漂移。管理员排除项仍保留在报告中。

## 5. 同分支分波施工

1. **Wave 0：文档契约（已完成，2026-07-16）**——冻结本规格、离线包 v1 与 ADR-0017；静态核对选择分支已消除、既有 ADR 一致且未扩张阶段 4 边界。
2. **Wave 1：契约与纯函数（已完成，2026-07-16）**——补 `TaskInfo.phase`、`finalize`、`RetryTaskRequest`、冲突决策模型；实现状态/阶段组合、内部重试检查点、完整校验证据、冲突前置条件、临时凭据零序列化，以及离线包 v1 canonical digest/路径/快照绑定/交叉引用纯函数。
3. **Wave 2：离线主路径（已完成，2026-07-16）**——实现 Nexus 原生数据目录发现器、自有目录包 Reader/Exporter、不可变计划快照、对象提交检查点和同二进制导出 CLI。
4. **Wave 3：在线与切换（已完成，2026-07-16）**——实现 REST 全量分页、npm packument 快照绑定、`awaiting-cutover` 门、`finalize` 最终扫描、重启后凭据补回和幂等续传。
5. **Wave 4：报告与向导真连（开发中）**——OpenAPI、devmock 和 Web 已接通任务控制、最终校验计数与在线漂移展示；计划生成的冲突决策提交与 Web 三决策交互仍需补齐，不能预标完成。
6. **Wave 5：严格门禁与真机验收（外部验收阻塞）**——2026-07-16 已在指定 Rust 1.96.1、5 GiB cgroup 容器完成 14 步严格质量门，OpenAPI 源码快照、前后端测试、两组浏览器 E2E、官方客户端与性能证据全部通过；仍须在本地 Docker Nexus OSS 3.70.x 上完成在线、原生目录、目录包三路径验收。

每一波都在同一分支提交其完整闭环，不并行维护长期分支；任何门禁失败立即阻断下一波。

## 6. 验收标准

- 状态机十态不变；`phase` 四值及合法组合穷举通过，`paused + awaiting-cutover` 不能通过 `resume` 绕过。
- `finalize` 仅接受在线 `paused + awaiting-cutover`；源持续写入时回到等待切换，零未处理漂移才可成功；删除只报告不传播。
- 重启导致临时凭据丢失；在线任务必须通过 `retry` 重新提交用户名/口令，随后从原检查点和原阶段恢复。数据库、快照、日志、响应和报告全量扫描无明文凭据。
- 异摘要冲突仅接受 `rename/map-empty/abandon`；三类决策同时进入不可变快照和报告，覆盖与未决决策均被阻断。
- 同一未变化来源二次执行迁移数为零，全部受支持对象计为同摘要跳过，无重复索引、无孤儿 blob。
- 原生数据目录使用已停止 Nexus 或一致性快照；`.properties`/`.bytes` 配对、deleted 跳过、归属、损坏分类和配置双来源均有测试。
- 本地 Docker Nexus OSS 3.70.x 覆盖 Raw/Maven/npm × hosted/proxy/group、分页、冲突、损坏、大文件、停写切换和服务重启；官方 curl、Maven、npm 客户端验证迁移后语义。
- 每一 Wave 均通过 ADR-0010 全量严格质量门；阶段 4 只有 Wave 5 完成后才可改为已完成。

## 7. 风险与固定约束

- 在线一致性依赖管理员执行停写/只读；工具以显式 `finalize` 和二次验证扫描阻止误报成功，但不控制 Nexus 写入模式。
- 原生数据目录只支持文件型 blob store；未知关键属性、缺失配对文件和损坏对象进入不可识别/损坏清单，不猜测归属。
- Nexus REST 分页必须以 continuation token 遍历并用稳定对象键去重；token 失效或重复必须显式失败或重试，禁止静默漏项。
- 迁移落库经过协议领域服务可能慢于直接搬运，但双校验、幂等和客户端可用性优先；不以绕过领域服务换取速度。
