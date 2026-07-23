# 功能规格：离线迁移包 v1 与导出（M1-FR-041 + FR-023 包格式本体）

> 状态：阶段 4 Wave 5 外部验收阻塞　·　关联 PRD：M1-FR-041、M1-FR-023　·　架构决策：ADR-0008、ADR-0015、ADR-0017

## 1. 定位与版本边界

JianArtifact 离线迁移包是可选的可移植、可整体校验归档，不取代 ADR-0015 确立的离线直读 Nexus 原生数据目录首选路径。v1 只支持 Nexus Repository OSS 3.70.x、Raw/Maven/npm 和文件型 blob store。

v1 的标准且唯一可直接读写形态是**目录树**。本阶段不接受 tar 作为输入，不生成 tar，不引入 `tar` crate；需要传输单文件时由运维在 JianArtifact 之外归档，使用前先恢复为目录树。

## 2. v1 目录树与清单

固定目录结构：

```text
{package-root}/
├─ manifest.json
├─ repositories/
│  └─ {repository-id}.json
├─ assets/
│  └─ {repository-id}/
│     └─ {shard-id}.jsonl
└─ blobs/
   └─ sha256/
      └─ {digest[0..2]}/
         └─ {64位小写十六进制摘要}
```

- `repository-id`、`shard-id` 是导出器生成的稳定 ASCII 标识；仓库显示名只存于 JSON，不直接充当文件名。
- `manifest.json` 字段固定为：`format_version`（固定整数 `1`）、`source_instance_id`、`snapshot_id`、`snapshot_time`、`nexus_version`、`repositories`、`object_count`、`total_bytes`、`overall_digest`。`repositories` 的每项固定包含 `repository_id`、`repository_path`、`asset_shards`、`object_count`、`total_bytes`；每个 `asset_shards` 项固定包含 `shard_id`、`path`、`object_count`、`total_bytes`。
- 每个 `repositories/*.json` 字段固定为：`source_instance_id`、`snapshot_id`、`snapshot_time`、`repository_id`、`name`、`format`、`repo_type`，以及按类型出现的 `member_repository_ids`、`upstream_url`、`upstream_credential_ref`。hosted 不携带后三项；proxy 必须有非空 `upstream_url`，可选凭据引用字符串；group 只携带有序 `member_repository_ids`。不适用或未提供的可选字段必须省略，禁止用显式 `null` 占位。明文上游凭据禁止进入包。
- 每个资产分片首行 header 固定包含 `source_instance_id`、`snapshot_id`、`snapshot_time`、`repository_id`；后续每行对象记录固定包含 `repository_id`、`object_key`、`size`、`content_type`、`sha256`、`blob_path`。对象记录必须与 header 仓库一致，并按 `object_key` UTF-8 字节序严格递增且唯一。
- blob 是原始字节，不做转码；路径中的摘要必须等于内容 sha256。每个 blob 至少被一个资产记录引用，不允许悬空 blob。
- 包根只允许 `manifest.json` 与上述三个目录；符号链接和其他特殊文件一律拒绝。

完整包判定必须同时满足：目录名不是 `.partial` 后缀、存在可解析的 `manifest.json`、所有索引文件存在、canonical digest 通过、交叉引用通过。缺少权威清单的裸 blob store 不属于 v1 包，必须回到迁移规格定义的降级路径。

## 3. canonical digest v1

### 3.1 字符与路径

- JSON、JSONL、路径记录、`source_instance_id`、`snapshot_id`、`snapshot_time` 全部使用严格 UTF-8；禁止 BOM 和非法 UTF-8。JSONL 只使用 LF（`0x0a`）换行。
- 参与摘要的路径是从包根开始的相对路径，分隔符固定为 `/`；禁止前导 `/`、反斜杠、空段、`.`、`..`、NUL 和盘符。路径比较不做大小写折叠，也不做 Unicode 归一化，直接比较其 UTF-8 字节。
- JSON 使用 v1 canonical JSON：无无意义空白；对象键按 UTF-8 字节序升序；数组顺序保留；数值只允许无前导零的十进制整数，禁止 `-0`；引号和反斜杠分别写为 `\"`、`\\`，退格/换页/LF/CR/制表分别写为 `\b`、`\f`、`\n`、`\r`、`\t`，其他 U+0000~U+001F 写为小写十六进制 `\u00xx`，其余字符直接写 UTF-8。仓库 JSON、资产 header/记录和 manifest 都按此规则输出；JSONL 每行是一个 canonical JSON 值且末行也以 LF 结束。

### 3.2 参与集合

摘要参与集合固定为：

1. canonical `manifest.json`，但**只排除顶层 `overall_digest` 字段**；其他字段全部参与。
2. manifest 仓库索引列出的全部 `repositories/*.json`。
3. manifest 仓库索引列出的全部 `assets/**/*.jsonl`。
4. 资产记录引用的全部 `blobs/sha256/**`。

目录本身不参与。参与集合之外出现任何普通文件，或 manifest 列出但缺失文件，均拒绝包；读取器不得忽略“无关文件”。同一 blob 被多个对象引用时只按其路径参与一次。

### 3.3 排序、分帧与算法

定义 `frame(bytes) = u64 大端长度 || bytes`，定义 `file_record = frame(path_utf8) || u64大端文件长度 || sha256(content_bytes)`。

`overall_digest` 的输入流固定为：

```text
frame("JIANARTIFACT-OFFLINE-PACKAGE")
|| frame("1")
|| frame(source_instance_id 的 UTF-8)
|| frame(snapshot_id 的 UTF-8)
|| frame(snapshot_time 的 UTF-8)
|| 按相对路径 UTF-8 字节序升序拼接的全部 file_record
```

其中：

- manifest 的 `content_bytes` 是移除顶层 `overall_digest` 后重新按 v1 canonical JSON 序列化的字节。
- repository/asset 文件的 `content_bytes` 是磁盘上的完整 canonical UTF-8 字节；blob 的 `content_bytes` 是原始字节。
- 最终值为上述输入流的 sha256，manifest 写成 `sha256:{64位小写十六进制}`。
- 文件长度是 `content_bytes` 的字节数，不是字符数。分帧和定长整数消除路径、字段和文件边界的拼接歧义。

### 3.4 跨快照绑定

`source_instance_id + snapshot_id + snapshot_time` 是快照绑定三元组。manifest、每个仓库配置和每个资产分片 header 必须逐字节相同；任一不一致立即拒绝。该三元组还作为 canonical digest 的独立前缀参与摘要，因此不能通过替换 manifest 绕过绑定。

blob 通过资产记录中的 sha256 与内容寻址路径绑定到该快照；整体摘要再绑定本快照的确切 blob 参与集合。来自其他快照的仓库文件、资产分片或不同内容 blob 会被三元组、整体摘要或逐对象 sha256 至少一层阻断。来自其他快照但字节与 sha256 完全相同的 blob 与当前内容等价，不构成可观察的跨快照差异。

## 4. 读取与校验顺序

读取器必须先完整校验，校验失败不创建迁移任务、不导入任何对象：

1. 校验根目录不是 `.partial`、结构与文件类型合法、`format_version == 1`、`nexus_version` 属于 3.70.x。
2. 校验所有文本为严格 UTF-8、所有路径满足 v1 规则、所有 JSON/JSONL 满足 canonical 规则。
3. 校验快照三元组一致、仓库索引与文件集合完全相等、无额外文件。
4. 按 §3 重算 `overall_digest` 并常量时间比较摘要值。
5. 校验 group 成员引用可解析、proxy 凭据只含引用名、资产对象键唯一且顺序合法。
6. 流式读取每个 blob，校验路径摘要、内容 sha256、大小及全部交叉引用；拒绝悬空引用和悬空 blob。
7. 全部通过后才把包暴露为可发现来源。

## 5. 同一二进制 CLI 导出

导出入口固定为同一 `jianartifact` 二进制的 CLI 子命令：

```text
jianartifact migration export \
  --source <NATIVE_DATA_DIR> \
  --repository-config <REPOSITORIES_JSON> \
  --nexus-version <3.70.x> \
  --output <PACKAGE_ROOT>
```

四个路径参数都相对于配置的 `migration-workdir` 解析；`source` 必须是 Nexus 原生文件型 blob store 数据目录，`repository-config` 必须是 canonical JSON 仓库配置文件，`output` 与其 `.partial` 同时受根目录、软链接和既存路径校验。当前导出器不连接在线 Nexus，因此 CLI 不接收用户名或口令；需要从在线 Nexus 获取仓库配置时，应在停写前单独生成配置文件，且不得把明文凭据写入该文件。

导出固定流程：

1. 要求目标 `<PACKAGE_ROOT>` 不存在，并在同一父目录创建 `<PACKAGE_ROOT>.partial`；若残留 `.partial` 已存在则拒绝覆盖并提示人工处理。
2. 在源端停写/只读窗口内枚举仓库配置、资产与正缓存，生成一次 `snapshot_id`，流式写 repository、asset 和 blob；大 blob 不整体入内存。
3. 对 `.partial` 中除 manifest 外的全部内容做交叉引用校验，生成 canonical manifest 内容并计算 `overall_digest`。
4. 把 manifest 写入 `.partial/manifest.json.tmp`，刷新文件内容后在同一目录原子重命名为 `manifest.json`；manifest 是目录内最后发布的文件。
5. 再次按读取器规则完整自校验；成功后把 `<PACKAGE_ROOT>.partial` 在同一文件系统原子重命名为 `<PACKAGE_ROOT>`，完成对外发布。
6. 任一步失败都保留或清理 `.partial`，但绝不生成最终目录；读取器无条件拒绝 `.partial`，因此中断产物不能被误当完整包。

proxy 上游凭据只导出引用名，不导出 Nexus 中的明文或可逆密文。CLI 进程退出即丢弃 Nexus 临时登录凭据。

## 6. 同分支施工与验收

- Wave 1 已于 2026-07-16 完成 manifest 模型、canonical JSON、路径规则、参与集合、快照绑定、交叉引用与 digest 纯函数测试。
- Wave 2 已于 2026-07-16 完成目录树 Reader/Exporter、`.partial` 原子发布、流式 blob、交叉引用校验，以及原生目录/配置文件在导出期间不变的复核。
- Wave 3 已于 2026-07-16 接通迁移任务检查点、报告和在线/离线统一执行路径。
- Wave 5 的指定 Rust 1.96.1、5 GiB cgroup 容器 14 步严格质量门已于 2026-07-16 通过；尚须在本地 Docker Nexus OSS 3.70.x 的停写窗口生成 v1 目录树，在断开源后导入并用 curl、Maven、npm 官方客户端验收。

必须覆盖：未知版本、非 3.70.x、非法 UTF-8、非法路径、非 canonical JSON、额外文件、缺失文件、manifest 排除规则错误、排序错误、整体摘要不符、快照三元组不符、悬空引用、悬空 blob、blob 内容损坏、导出中断、manifest 原子发布前后崩溃、5 GiB 流式样本、包内及日志凭据零泄露。

每一 Wave 通过 ADR-0010 完整严格质量门后才可进入下一波。v1 的目录树、摘要算法和读取拒绝条件发布后不可静默修改；任何不兼容变化必须提升 `format_version`。

## 7. 风险与固定约束

- 导出一致性依赖源端停写/只读；CLI 不替管理员控制 Nexus 写入模式，自校验不能补救持续变化的源。
- v1 不解析 Nexus 内嵌 H2/OrientDB；配置来自 REST，原生数据目录首选路径的手动补配置仍由迁移规格处理。
- 裸 blob store 没有权威清单与整体摘要，只能走降级恢复，不能伪装为 v1 包或给出全量成功结论。
- tar、压缩、加密封装和其他格式扩展均不属于阶段 4。
