# IDE1_TASE v1.1 ↔ IME1.0 映射说明

| 属性 | 内容 |
|---|---|
| 文档编号 | `MAP-TASE-IME1-001` |
| 版本 / 日期 | **v1.1 / 2026-11-05**（对应 TASE v1.1 / 2026-11-05） |
| 状态 | **文档对照稿（不含任何实现）** |
| 来源 | 本次交付的 IDE1.0 TASE v1.1 设计，共 5 份 Markdown + 1 份 xlsx（清单见 `../README.md`） |
| 目的 | 把 TASE 设计中出现的**每一个 crate / 模块**映射到本仓库（IME1.0）的实际落点，分成「已有可复用 / 可适配 / 必须新建」三类，并给出 M0→M4 里程碑现状与已知落差 |
| 不含 | 不修改 TASE 正文；不修改 `01_Source/**`；不引入设计之外的实现方案 |

---

## 0. 前提与读法

1. **TASE v1.1 是为另一个仓库写的。** 其适用项目是 <https://github.com/UlyssesLeoLee/IDE1.0>（见 `IDE1_TASE_Requirements_v1.1.md:21-27`），假定该仓库存在 `ide-kernel` / `editcore` / `ide-shell-protocol` 等 crate。
2. **本仓库 IME1.0 没有这些 crate。** 本仓库的对应物是 `pime-core`（文本/Document 内核）、`pime-agent`（MCP/CLI/管道/HTTP 前端）、`pime-ipc`（带 ACL 的命名管道）、`pime-tsf`（Windows TSF）、`pime-setup`、`pime-tray`、`pime-ai-worker`。
3. **因此，TASE 不能在本仓库「照单施工」。** 下文把 TASE 的每个 crate 落到本仓库的真实文件上，并明确指出哪些是**等价可复用**、哪些只是**同名近似**、哪些必须**新建**。
4. 本文件中的 TASE 出处用 `文件名:行号` 标注，便于回查原文；TASE 正文一律未改写。
5. 仓库现状引用一律给出 `路径:行号`，便于复核。

---

## 1. 总表：TASE 的每个 crate / 模块 → IME1.0 落点

| TASE 侧 crate / 模块 | TASE 中的位置 | 类别 | IME1.0 落点 |
|---|---|---|---|
| `ide-kernel` | RD §1.2；BD §2.3；DD §1.1 / §8.1 / §17.1 | 已有可复用（有差距，见 §2.1、§7-G1） | `01_Source/crates/pime-core/src/edit/transaction.rs`、`textrange.rs`、`session.rs` |
| `ide-kernel-core` | RD §1.2（仅出现在 Cargo.toml 包清单） | 已有可复用（角色未在设计中展开） | `pime-core`（`pime-core/src/lib.rs` 为其聚合层） |
| `editcore` | RD §1.2；BD §2.3 | 已有可复用（已被 `pime-core` 吸收） | `pime-core/src/textrange.rs`、`json.rs`、`sha256.rs`、`base64.rs` 等底层工具 |
| `ide-shell-protocol` | RD §1.2；BD §2.3 / §4.1；DD §1.1 / §2.7 / §8.1 / §10.1 | 已有可复用（有差距，见 §2.2、§7-G2） | `01_Source/crates/pime-agent/src/{cli,serve,mcp,core}.rs` + `pime-core/src/{protocol,agent_proto}.rs` |
| `ide-cli` | RD §1.2（仅包清单） | 无直接对应 | `pime-agent/src/cli.rs`（最接近） |
| `ide-shell` / `ide-shell-web` / `ide-shell-desktop` | RD §1.2（仅包清单） | 无直接对应 | 桌面壳 ≈ `pime-tray`；Web 壳本仓库**不存在**（见 §7-G3） |
| `ide-fast` | RD §1.2（仅包清单） | 无对应 | **无**；设计中未展开其职责，落地前需确认（见 §7-G9） |
| `tase-contract` | BD §2.3；DD §1.1 | **必须新建** | `01_Source/crates/tase-contract/` |
| `tase-registry` | BD §2.3；DD §1.1 | **必须新建** | `01_Source/crates/tase-registry/` |
| `tase-planner` | BD §2.3；DD §1.1 | **必须新建** | `01_Source/crates/tase-planner/` |
| `tase-semantic` | BD §2.3；DD §1.1 | **必须新建** | `01_Source/crates/tase-semantic/` |
| `tase-executor` | BD §2.3；DD §1.1 / §8.1 | **必须新建** | `01_Source/crates/tase-executor/` |
| `tase-worker` | BD §2.3；DD §1.1 / §14 | **必须新建** | `01_Source/crates/tase-worker/` |
| `tase-metrics` | BD §2.3；DD §1.1 / §9 | **必须新建** | `01_Source/crates/tase-metrics/` |
| `tase-local-hub` | DD §17.1 / §17.2 | **必须新建** | `01_Source/crates/tase-local-hub/` |
| `tase-local-ipc` | DD §17.1 / §17.3 | **必须新建（功能与 `pime-ipc` 重叠，见 §7-G4）** | `01_Source/crates/tase-local-ipc/` |
| `tase-agent-scheduler` | DD §17.1 / §17.5 | **必须新建** | `01_Source/crates/tase-agent-scheduler/` |
| `tase-local-store` | DD §17.1 / §18 | **必须新建** | `01_Source/crates/tase-local-store/` |
| `tase-catalog` | DD §17.1 / §19 | **必须新建** | `01_Source/crates/tase-catalog/` |
| `tase-catalog-mcp` | DD §17.1 / §19.6 | **必须新建** | `01_Source/crates/tase-catalog-mcp/` |
| `tase-ime-core` | DD §17.1 / §20 | **必须新建** | `01_Source/crates/tase-ime-core/` |
| `tase-ime-tsf` | DD §17.1 / §20 | **必须新建，但与既有 `pime-tsf` 是「扩展」关系（见 §7-G5）** | `01_Source/crates/tase-ime-tsf/`（或并入 `pime-tsf`，待裁定） |
| `web-router`（Next.js App Router） | BD §2.3；DD §1.1 `apps/web/` | 无对应 | **无**；本仓库无 Web 前端（见 §7-G3） |
| `templates/`（模板工件树） | BD §8；DD §1.1 `templates/rust/handler-create/1.0.0/` | 无对应 | **无**；本仓库无模板目录（见 §7-G6） |
| `tase-safe-template` / `tase-template`（模板**引擎标识**，非 crate） | DD §3.1；DD §19.1 | 无对应 | 随 manifest 进入 `tase-registry` 的 digest 与策略校验（见 §5） |
| `infra/migrations/0001_tase.sql`、`infra/policies/tase-policy.yaml` | DD §1.1 | 无对应 | **无**；本仓库无 SQL 迁移与策略文件体系（见 §7-G7） |

---

## 2. 已有、可复用

### 2.1 `ide-kernel` / `editcore` → `pime-core`

**TASE 里的角色**：`ide-kernel` 是「唯一文本修改内核」，以 `Document`/`Session` 聚合 `editcore`，所有正式文档变更经 `Document::apply` 记录撤销，位置语义为字符计数（`IDE1_TASE_Requirements_v1.1.md:22`；`IDE1_TASE_Basic_Design_v1.1.md:16,76-77`）。`editcore` 负责文本、undo、range、encoding，被明确要求「不另建文本引擎」（`IDE1_TASE_Basic_Design_v1.1.md:77`）。

**本仓库实际有的**：

| TASE 期望的能力 | 本仓库实现 | 位置 |
|---|---|---|
| 计划 → 准备 → 写入 → 落账的一次性事务 | `Coordinator::create_plan` / `prepare` / `begin_apply` / `finish_apply` / `invalidate` / `discard` / `purge` | `pime-core/src/edit/transaction.rs:227,330,373,427,277,488,501` |
| 写入前必须有的三重闸门（用户确认、权限代理、语法校验） | `PrepareArgs { confirmed, grant_ok, valid }` | `pime-core/src/edit/transaction.rs:175-181` |
| 文档版本变化使旧计划失效 | `Coordinator::invalidate` + `DocState.version` | `pime-core/src/edit/transaction.rs:276-277, 92` |
| 一次写入只能成功一次（重放不重复写） | `begin_apply` 标记令牌已用 | `pime-core/src/edit/transaction.rs:371-373` |
| 文本范围 / 偏移工具 | `textrange` 模块 | `pime-core/src/textrange.rs` |
| 会话 | `session` 模块 | `pime-core/src/session.rs` |
| 字符位置编辑（对应 `editcore`） | `textrange` + `json`/`sha256`/`base64` 等底层工具 | `pime-core/src/{textrange,json,sha256,base64}.rs` |

**可直接沿用的设计结论**：TASE 的「单一编辑内核、禁止另建第二份编辑事实源」（`IDE1_TASE_Basic_Design_v1.1.md:16`；`IDE1_TASE_Detailed_Design_v1.1.md:839`）在本仓库**天然成立**——`pime-core` 已经是唯一内核。

**必须先解决的两处不等价**（详见 §7-G1、§7-G8）：

1. **没有 `Document::apply` 这个入口。** 本仓库等价物是上表的 `Coordinator` 四步协议，而且**文本写入由宿主编辑器执行，core 自身不持有 buffer、不持有 undo 栈**（`pime-core/src/edit/transaction.rs:556` 的 undo 栈属于测试替身 `FakeDoc`）。TASE 里「一个 Plan 呈现为单一逻辑 undo 事务」（`IDE1_TASE_Detailed_Design_v1.1.md:622`）在本仓库**不能靠 `pime-core` 达成**，只能靠宿主编辑器的单次 `applyEdit`。
2. **位置语义不同。** 本仓库的编辑偏移是 **UTF-16 code unit**（`pime-core/src/edit/transaction.rs:226`），TASE 假设 `ide-kernel` 是 **char-based**（`IDE1_TASE_Basic_Design_v1.1.md:259`），而 TASE 的 `PositionMapper` 又要同时处理 char / UTF-8 byte / UTF-16。落本仓库时，**PositionMapper 必须以 UTF-16 为中间基准**，不能直接照抄 TASE 的 char 假设。

### 2.2 `ide-shell-protocol` → `pime-agent`（+ `pime-core::protocol` / `agent_proto`）

**TASE 里的角色**：已有 JSON-RPC 2.0 风格的 HTTP/WS/stdio 接入与 Buffer / Provider trait；新接口**加命名空间而不是替换旧接口**（`IDE1_TASE_Requirements_v1.1.md:23`）。TASE 要在其上扩展 `tase.*` dispatch，并保留旧的标准错误码 `-32700/-32600/-32601/-32602/-32603`（`IDE1_TASE_Detailed_Design_v1.1.md:246`）。

**本仓库实际有的**：

| TASE 期望 | 本仓库实现 | 位置 |
|---|---|---|
| stdio 上的 JSON-RPC / MCP | `pime-mcp` 二进制，`tools/list` + `tools/call` | `pime-agent/src/mcp.rs:294-295`、`pime-agent/src/bin/pime-mcp.rs` |
| MCP 工具集 | `ime_insert_text`、`ime_read_selection`、`ime_set_language`、`ime_candidates`、`ime_predict`、`ime_validate`、`ime_curate_dictionary` | `pime-agent/src/mcp.rs:77,102,108,119,138,163,188` |
| HTTP 接入 | 管道承载的 HTTP 服务：`GET /health`、`POST /`，Bearer 令牌鉴权 | `pime-agent/src/serve.rs:36-38,45-47` |
| 核心 JSON-RPC dispatch | `pime_core::protocol::{Core, dispatch, handle_line, serve}` | `pime-core/src/protocol.rs:317,334,397,459` |
| Agent 端请求/应答编解码 | `pime_core::agent_proto::{Request, Reply, Proposal}`，6 个 `Op` | `pime-core/src/agent_proto.rs:98,186,295,445` |
| 标准错误码 | `PARSE_ERROR=-32700`、`INVALID_REQUEST=-32600`、`METHOD_NOT_FOUND=-32601` | `pime-agent/src/mcp.rs:23,26,29` |

**适配方式**（不破坏既有面）：
- TASE 的 `tase.*` 方法**新增命名空间**，与现有 `ime_*` MCP 工具并存，符合 TASE 自己的「加命名空间而不是替换」原则。
- 现有 `pime_core::protocol::dispatch` 是最自然的扩展点：新增 `tase.*` 分支即可，旧的 `ime_*` 分支不动。
- 旧错误码兼容面**比 TASE 假设的窄**：本仓库可见 `-32700/-32600/-32601` 三码，`-32602/-32603` 未见定义（`pime-agent/src/mcp.rs:23-29`）。若要严格满足 `IDE1_TASE_Detailed_Design_v1.1.md:246` 的「保留五个码」，需要在 `pime-agent` 侧补齐定义。

### 2.3 命名管道 → `pime-ipc`

**TASE 里的角色**：同机 Agent / IDE / IME ↔ Local Hub 的传输层，要求：Windows Named Pipe、SDDL 绑定当前 logon SID、显式 `PIPE_REJECT_REMOTE_CLIENTS`、拒绝 remote client、4 字节小端 `u32` 长度前缀 + UTF-8 JSON、默认单帧上限 1 MiB、每连接独立 session（`IDE1_TASE_Basic_Design_v1.1.md:395,400`；`IDE1_TASE_Detailed_Design_v1.1.md:851`）。

**本仓库实际有的**：

| TASE 期望 | 本仓库实现 | 位置 |
|---|---|---|
| 命名管道 + 拒绝远程客户端 | `PIPE_TYPE_BYTE \| PIPE_READMODE_BYTE \| PIPE_WAIT \| PIPE_REJECT_REMOTE_CLIENTS` | `pime-ipc/src/pipe.rs:149` |
| DACL 绑定当前用户 | `SecurityDescriptor::for_current_user()`（SDDL `D:P(A;;mask;;;SID)`） | `pime-ipc/src/winsec.rs:141-142` |
| 取当前用户 SID | `current_user_sid_string()` | `pime-ipc/src/winsec.rs:78` |
| 取终端服务会话号 | `current_session_id()` | `pime-ipc/src/winsec.rs:206` |
| 复核管道实际 ACL | `dacl_sddl_of_handle()` | `pime-ipc/src/winsec.rs:166` |
| 分帧 / 握手 / 连接 | `frame.rs`（`HEADER_LEN=8`、`MAGIC="PI"`、`CONTROL_FRAME_MAX=64 KiB`、`BULK_FRAME_MAX=2 MiB`、`HANDSHAKE_FRAME_MAX=4 KiB`）、`handshake.rs`、`connection.rs`、`endpoint.rs`、`auth.rs` | `pime-ipc/src/frame.rs:17-25` |

**可直接沿用**：安全基线（显式 DACL + 拒绝远程客户端 + 会话号）已经比 TASE 的文字要求更严格，**不需要重写传输层**。

**必须先解决的两处不等价**（详见 §7-G4）：
1. **帧格式不同**：本仓库是 8 字节头（含 2 字节 magic `"PI"`），TASE 是 4 字节小端 `u32` 长度前缀。若 Hub 与 Agent 分别实现，**必须二选一**（建议以 `pime-ipc` 为准并改 TASE 侧假设）。
2. **ACL 主体不同**：本仓库绑「当前用户 SID」，TASE 绑「**logon SID**」并要按 `\\.\pipe\ide1-tase-{logon-sid}` 命名空间隔离多登录会话。本仓库的 `current_session_id()` 只**记录**会话，未参与命名空间或鉴权。

### 2.4 TSF → `pime-tsf`

**TASE 里的角色**：Windows 自定义 IME 必须使用 TSF；IME DLL 只做 COM 适配 / 快捷键 / 候选窗口，**不得**包含 Worker、LLM、网络凭证或持久仓库写入；候选必须携带 `asset_id@version`；「展示 ≠ 写入」，项目级改动必须转交 IDE 服务走 Preview/审批（`IDE1_TASE_Basic_Design_v1.1.md:529,535`；`IDE1_TASE_Detailed_Design_v1.1.md:1208-1263`）。

**本仓库实际有的**：

| 能力 | 实现 | 位置 |
|---|---|---|
| TSF 文本服务注册 | `register.rs` | `pime-tsf/src/register.rs` |
| 输入法控制器 / 会话 | `controller.rs`、`ime_switch.rs` | `pime-tsf/src/controller.rs`、`ime_switch.rs` |
| 候选窗口（激光面板） | `laser.rs`、`laser_candidates.rs`、`laser_ui.rs`、`candidate_skin.rs`、`popup.rs` | `pime-tsf/src/laser*.rs` |
| 本地预测 / 词典 | `predictor.rs`、`pime-core::cjk` | `pime-tsf/src/predictor.rs` |
| 快捷键 | `keymap.rs` | `pime-tsf/src/keymap.rs` |
| Agent → IME 的桥 | `agent_bridge.rs`（窗口消息 `WM_COPYDATA`） | `pime-tsf/src/agent_bridge.rs:8,186` |
| UI Automation / COM | `uielement.rs`、`uielement_com.rs` | `pime-tsf/src/uielement*.rs` |

**可直接沿用**：TASE 的「IME DLL 只做 COM 适配/候选窗、不得含 LLM 与网络凭证」这条硬约束，本仓库的 `pime-tsf` **已经满足**——候选计算在 `pime-core::predict` / `pime-core::cjk`，模型调用在 `pime-ai-worker`，两边不混。

**必须先解决的落差**（详见 §7-G5）：TASE 的 `tase-ime-tsf` 是「候选编排 + Hub IPC client」的薄适配层，而 `pime-tsf` 是一个**完整可用的输入法**。二者是**扩展关系**而非并行新建——再建一个 TSF DLL 会与现有注册冲突。

---

## 3. 本仓库没有、但可以适配（TASE 中直接点名既有 crate 的位置）

下表逐条列出 TASE 正文中**直接引用** `ide-kernel` / `editcore` / `ide-shell-protocol` 的地方，及其在本仓库的落点。

| TASE 出处 | TASE 的说法 | 在 TASE 中承担的角色 | 落到本仓库 | 适配要点 |
|---|---|---|---|---|
| `IDE1_TASE_Requirements_v1.1.md:21` | 工作区 Cargo.toml 包版本 0.3.9，列出 `ide-kernel`、`ide-kernel-core`、`ide-cli`、`ide-shell`、`ide-shell-web`、`ide-shell-desktop`、`ide-shell-protocol`、`editcore`、`ide-fast` | 前置事实声明 | `pime-core`、`pime-agent`、`pime-tray`、`pime-setup` | 包名整体不同；`ide-shell-web` / `ide-fast` 无对应物 |
| `IDE1_TASE_Requirements_v1.1.md:22` | `ide-kernel` 以 `Document`/`Session` 聚合 `editcore`，编辑须经统一 `Document::apply` 记录撤销，位置为字符计数 | 前置事实声明 | `pime-core`（`edit/transaction.rs`、`session.rs`） | **无 `Document::apply`**；写入与 undo 在宿主编辑器（§7-G1） |
| `IDE1_TASE_Requirements_v1.1.md:23` | `ide-shell-protocol` 已有 JSON-RPC 2.0 风格 HTTP/WS/stdio 与 Buffer/Provider trait；新接口加命名空间 | 前置事实声明 | `pime-agent` + `pime-core::protocol` | 有 stdio/HTTP，**无 WS**；无 Buffer/Provider trait 对应物 |
| `IDE1_TASE_Requirements_v1.1.md:118`（FR-016） | 通过 `ide-kernel` 统一编辑接口执行正式文档更改，保持 undo/redo 语义 | P0 功能要件 | `pime-core::edit::transaction` + 未来 `tase-executor` | 需要一层等价 `DocumentEditAdapter`（DD 已声明是新增接口） |
| `IDE1_TASE_Requirements_v1.1.md:182`（NFR-011） | 不破坏 `ide-shell-protocol` 现有方法及 `ide-kernel` 文档变更语义 | 兼容要件 | `pime-agent` / `pime-core::protocol` | 既有面是 `ime_*` 命名空间，兼容面比 TASE 假设的小 |
| `IDE1_TASE_Requirements_v1.1.md:232`（D-05） | `ide-kernel` 是否可直接支撑多文件 Git 事务 —— 待开发适配层 | 未决风险项 | `pime-core` + `tase-executor` | **本仓库没有 Git 集成层**，D-05 在此更靠前且仍未决 |
| `IDE1_TASE_Basic_Design_v1.1.md:16`（原则 3） | 所有正式文档变更经既有 `ide-kernel::Document::apply` 或其标准事务适配层 | 设计原则 | `pime-core` | 同 §7-G1 |
| `IDE1_TASE_Basic_Design_v1.1.md:17`（原则 4） | 增加 `tase.*` RPC，适配 HTTP/WS/stdio；不破坏 `ide-shell-protocol` 的 Vim Buffer API | 设计原则 | `pime-agent` | 本仓库无 Vim Buffer API，也无 WS |
| `IDE1_TASE_Basic_Design_v1.1.md:49` | 架构图节点 `K[ide-kernel + editcore]` | 架构图 | `pime-core` | 图需重绘 |
| `IDE1_TASE_Basic_Design_v1.1.md:76-78` | 三行既有 crate 的职责与「复用/新增」判定 | 模块边界表 | §1 总表 | 本表即映射依据 |
| `IDE1_TASE_Basic_Design_v1.1.md:259`（BD §7.2） | `ide-kernel` 用 char-based 行列；需唯一 `PositionMapper` 做 char/byte/UTF-16 转换 | 位置语义 | `pime-core::textrange` + `tase-semantic/src/position.rs` | **本仓库是 UTF-16 偏移**，映射基准必须改成 UTF-16（§7-G8） |
| `IDE1_TASE_Basic_Design_v1.1.md:337`（§14） | `Document::apply` 与 `ide-shell-protocol` 当前版本必须 PoC 确认多文件事务适配点 | 风险条目 | — | 该 PoC **仍未做**，见 §7-G1 |
| `IDE1_TASE_Detailed_Design_v1.1.md:17-19`（DD §1.1） | 三个既有 crate 在 workspace 结构中的位置 | 分包结构 | `01_Source/crates/pime-*/` | 目录结构图需按本仓库重画 |
| `IDE1_TASE_Detailed_Design_v1.1.md:44`（DD §1.1 注） | `executor` 依赖它们并以适配接口调用 `ide-kernel`；`ide-shell-protocol` 只引用 contract | **依赖方向约束** | `tase-executor` → `pime-core`；`pime-agent` → `tase-contract` | 依赖方向可**原样保留**，这是 TASE 最可移植的一条 |
| `IDE1_TASE_Detailed_Design_v1.1.md:69,75`（DD §1.2） | `DocumentEditAdapter` 是本设计新增的适配接口，非宣称 `ide-kernel` 已有同签名 | 接口适配 | `tase-executor/src/` 内新增 | 需对 `pime-core::edit::transaction` 做一次 PoC 对接 |
| `IDE1_TASE_Detailed_Design_v1.1.md:246`（DD §2.7） | 保留现有 `ide-shell-protocol` 的 `-32700/-32600/-32601/-32602/-32603` | 协议兼容 | `pime-agent/src/mcp.rs:23-29` | 本仓库见三码，`-32602/-32603` 未见定义 |
| `IDE1_TASE_Detailed_Design_v1.1.md:622`（DD §8.1） | `Tauri/CLI -> ide-shell-protocol -> tase-executor -> DocumentEditAdapter -> ide-kernel::Document`；一个 Plan 是单一逻辑 undo 事务 | 本地 IDE 模式链路 | `pime-agent` → `tase-executor` → `pime-core` | 本仓库**无 Tauri/Web UI**，链路起点只有 CLI / HTTP / stdio（§7-G3） |
| `IDE1_TASE_Detailed_Design_v1.1.md:715`（DD §10.1） | 旧 `ide-shell-protocol` 端点回归通过 | 发布门禁 | `pime-agent` / `pime-core::protocol` | 对应回归面是 `ime_*` MCP 工具与 `pime-core::protocol` 用例 |
| `IDE1_TASE_Detailed_Design_v1.1.md:839`（DD §17.1 尾注） | 保持 `ide-kernel` 单一编辑事实源；Hub 调用既有执行器，不直接实现第二套文本编辑缓冲 | 架构硬约束 | `pime-core` + `tase-local-hub` | 本仓库已满足；但落地时要防止 `tase-local-hub` 自己持有 buffer |

---

## 4. 必须新建的 15 个 crate

落点路径统一为 `01_Source/crates/<name>/`，目录结构**照抄** TASE 的 DD §1.1 与 DD §17.1（本仓库 workspace 根目录为 `01_Source/`，非 TASE 的仓库根）。

| # | crate | 职责（一句话） | 依赖谁 | 落点文件（按 DD §1.1 / §17.1） |
|---:|---|---|---|---|
| 1 | `tase-contract` | API DTO、JSON Schema、契约版本与业务错误码——最底层，不依赖任何其它 TASE crate | 无（仅标准库 + 序列化库） | `01_Source/crates/tase-contract/src/{lib,api,error,ids,version}.rs`<br>`01_Source/crates/tase-contract/schemas/{plan,template,job,preview}.schema.json` |
| 2 | `tase-registry` | 模板索引、内容哈希（digest）、许可与签名政策、模板失配判定 | `tase-contract` | `01_Source/crates/tase-registry/src/{manifest,loader,policy,query,digest,validator}.rs` |
| 3 | `tase-semantic` | 语法树适配、符号索引、引用与影响面计算、char/byte/UTF-16 位置映射 | `tase-contract` | `01_Source/crates/tase-semantic/src/{lib,adapter,symbol,position,rust,typescript,impact}.rs` |
| 4 | `tase-planner` | 意图策略、上下文裁剪、计划校验、`plan_digest` | `tase-contract`、`tase-registry`、`tase-semantic` | `01_Source/crates/tase-planner/src/{strategy,context,validator,plan_hash}.rs` |
| 5 | `tase-executor` | Preview / Apply / Rebase / 冲突判定 / 工作区快照 / Git CAS 提交 / 撤销 | `tase-contract`、`tase-registry`、`tase-planner`、`tase-semantic`、`pime-core`（编辑适配） | `01_Source/crates/tase-executor/src/{preview,apply,rebase,conflict,workspace,git_tx,undo}.rs` |
| 6 | `tase-worker` | 队列消费者、租约与 fencing、沙箱执行、编译/测试验证、故障恢复 | `tase-executor`、`tase-metrics`、`tase-local-store`（本地形态） | `01_Source/crates/tase-worker/src/{main,consumer,lease,runner,verifier,recovery}.rs` |
| 7 | `tase-metrics` | Token 与成本度量、延迟、ACI 验收指标、追踪与统计 | `tase-contract` | `01_Source/crates/tase-metrics/src/{meter,pricing,aci,telemetry}.rs` |
| 8 | `tase-local-hub` | 常驻 Local Agent Hub 二进制与运行时：启动单例、鉴权、会话与授权、工作区、事件流 | `tase-local-ipc`、`tase-agent-scheduler`、`tase-local-store`、`tase-catalog`、`tase-contract` | `01_Source/crates/tase-local-hub/src/bin/tase-local-hub.rs`<br>`01_Source/crates/tase-local-hub/src/{bootstrap,server,auth,session,workspace,telemetry}.rs` |
| 9 | `tase-local-ipc` | Named Pipe / UDS 传输、长度前缀分帧、连接管理、ACL 与协议协商 | `tase-contract`（**实现上应优先复用 `pime-ipc`**，见 §7-G4） | `01_Source/crates/tase-local-ipc/src/{transport,framing,connection,acl}.rs` |
| 10 | `tase-agent-scheduler` | 每 Agent 公平队列（加权轮转）、资源配额、租约与预算、事件分发 | `tase-contract` | `01_Source/crates/tase-agent-scheduler/src/{queues,quota,leases,budget,events}.rs` |
| 11 | `tase-local-store` | SQLite WAL 持久化、迁移、outbox、快照、崩溃后对账 | `tase-contract` | `01_Source/crates/tase-local-store/src/{sqlite,migrations,reconcile,snapshots}.rs` |
| 12 | `tase-catalog` | 模式/算法知识库（GoF 23 + 工程模式 20 + 算法 50）、别名检索、约束、推荐排序、治理与实现认证状态 | `tase-contract` | `01_Source/crates/tase-catalog/src/{assets,index,recommend,governance,implementation}.rs` |
| 13 | `tase-catalog-mcp` | 把 Catalog 与 TASE 应用投影为 JSON-RPC 方法与 MCP 工具，与内部共用同一套 DTO | `tase-contract`、`tase-catalog` | `01_Source/crates/tase-catalog-mcp/src/{tools,schema,adapter}.rs` |
| 14 | `tase-ime-core` | 跨平台候选排序与上下文隐私过滤（**不依赖 COM**，可在非 Windows 构建） | `tase-catalog`、`tase-contract` | `01_Source/crates/tase-ime-core/src/{candidate,context,rank,policy}.rs` |
| 15 | `tase-ime-tsf` | Windows TSF 适配 DLL：COM 适配、composition、候选 UI、Hub IPC client | `tase-ime-core`、`tase-local-ipc` | `01_Source/crates/tase-ime-tsf/src/{com_adapter,composition,candidate_ui,ipc_client}.rs` |

**依赖方向（照抄 `IDE1_TASE_Detailed_Design_v1.1.md:44` 并补上本仓库对应物）**：

```text
tase-contract  ←  tase-registry / tase-semantic / tase-planner / tase-metrics
                                   ↓
                            tase-executor  ←── pime-core（编辑适配，不反向依赖）
                                   ↓
                 tase-worker  |  tase-local-hub
                                   ↓
        tase-local-ipc / tase-agent-scheduler / tase-local-store / tase-catalog
                                   ↓
                tase-catalog-mcp / tase-ime-core → tase-ime-tsf
```

**平台条件编译要求**（`IDE1_TASE_Detailed_Design_v1.1.md:839`）：`tase-ime-tsf` 编译/安装独立，**不强制把 COM DLL 加进现有全平台默认 workspace**。本仓库 `pime-tsf` 已在做同样的分离，落地时应沿用现有做法，并检查 `rust-version` 与 CI 兼容性。

---

## 5. 非 crate 的落点

| TASE 交付物 | TASE 位置 | 本仓库落点 | 说明 |
|---|---|---|---|
| REST `/api/v1/tase/{templates,plans,previews,applications,jobs}` | DD §1.1 `apps/web/`；BD §4.2 | `pime-agent/src/serve.rs` | 本仓库 HTTP 只有 `GET /health` 与 `POST /`，**没有**路径路由层；需新增路由或改用单一 POST + `method` 字段 |
| JSON-RPC `tase.template.search` / `tase.code.plan` / `tase.code.preview` / `tase.code.apply` / `tase.job.get` / `tase.job.cancel` / `tase.metrics.get` | BD §4.1/§4.2 | `pime-core/src/protocol.rs`（dispatch 分支）+ `pime-agent/src/mcp.rs` | 与现有 `ime_*` 工具并存 |
| MCP 工具 `tase_catalog_search` / `_inspect` / `_recommend` / `_instantiate` / `_verify`、`tase_code_apply` | BD §17.5；DD §19.6 | `tase-catalog-mcp/src/tools.rs`（新建），经 `pime-agent` 暴露 | 本仓库 MCP 工具**有 `inputSchema`、无 `outputSchema`、无 `annotations`**（`pime-agent/src/mcp.rs:242`）；TASE §19.6 要求三者齐备 |
| 本地 IPC RPC `tase.agent.register` / `.heartbeat` / `.unregister`、`tase.workspace.snapshot`、`tase.local.status` / `.events` | DD §17.3 | `tase-local-hub` + `tase-local-ipc` | 现有 `pime-agent::serve` 的管道只服务 HTTP，未做 JSON-RPC 帧 |
| 模板工件树 `templates/rust/handler-create/1.0.0/{manifest.yaml,params.schema.json,fragments,fixtures,tests}` | BD §8；DD §1.1 | **无对应目录**，需新建 | 仓库目前无模板资产体系 |
| 模板引擎标识 `tase-safe-template`（模板 manifest 的 `engine`）与 `tase-template`（目录资产 `implementations[].engine`） | DD §3.1（`IDE1_TASE_Detailed_Design_v1.1.md:257`）；DD §19.1（`IDE1_TASE_Detailed_Design_v1.1.md:1081`） | `tase-registry`（引擎名 + `engine_version` 一并参与 digest） | **这两个不是 crate，而是 manifest 里的引擎标识符**。TASE 要求「相同模板哈希与参数在相同引擎版本应得到相同输出」（`IDE1_TASE_Basic_Design_v1.1.md:279`），即 `digest + template_version + engine_version` 三元组是重现结果的凭据（BD §3.2）。落地时：引擎名与版本必须进 digest、必须可 pin；「禁用宏/表达式/任意脚本的受限模板语言」这条安全属性必须落到 `tase-registry` 的策略校验里 |
| PostgreSQL 参考 DDL（`plans` / `jobs` / `leases` / `commits` / `audit_events` / `cost_records` / `symbol_index`） | DD §6.1 | **无对应物**，仓库无 SQL 层 | 见 §7-G7 |
| SQLite WAL 参考 DDL（本地库） | DD §18.1 | `tase-local-store/src/sqlite.rs`（新建） | 仓库**全无 SQLite 依赖**（`01_Source` 内 grep `sqlite` 无命中） |
| `catalog_assets` / `catalog_implementations` 表 | DD §19.5 | `tase-local-store/src/migrations.rs` | 同上 |
| 模式/算法资产清单（93 项） | RD §14；DD §19.4 | `tase-catalog` + `tase-local-store` | 属设计资产清单，**不是已认证实现**（见交付说明第 15 行） |
| `next.js` Web Router / OIDC / 多租户 | BD §2.2、§13 | **无对应物** | 见 §7-G3 |

---

## 6. M0 → M4 里程碑现状

**验收重点照抄自** `IDE1_TASE_Detailed_Design_v1.1.md:1326-1336`（DD §23 发布与实现优先级）——「必须完成」与「验收重点」两列是 TASE 原文，**未改写**。第三列为本仓库现状（整理时观察，非 TASE 原文）。

| 阶段 | 必须完成（TASE 原文） | 验收重点（TASE 原文） | 本仓库现状（2026-11-05 观察） |
|---|---|---|---|
| **M0** | `Contract v1.1`、旧接口兼容层、知识卡 Schema、Local Hub 安全基线 | Schema/ACL/授权测试 | ACL 基线**已有**（`pime-ipc` 的 DACL + `PIPE_REJECT_REMOTE_CLIENTS`）；`Contract v1.1`、知识卡 Schema、Local Hub **均无** |
| **M1** | 常驻 Hub、多进程注册、Rust Catalog/Plan/Preview/Apply、最低限本地持久化 | 同机多个 Agent 读写隔离、离线工作 | Catalog/Plan/Preview/Apply **全无**；多进程隔离**部分具备**（`pime-ipc` 多实例 + 会话号）；持久化为 JSON/Markdown，非 SQLite |
| **M2** | 公平队列、锁与版本前置条件、恢复、GoF 23/工程模式/算法目录、MCP 工具 | 64 连接、16 并行准备、冲突与算法测试 | 队列**无**（`pime-core::scheduler` 是输入法 profile 锁，不是任务队列）；目录与 `tase_*` MCP 工具**无** |
| **M3** | Windows TSF IME、IDE 统一候选、F1、Vercel Router、可视化与成本基准 | 输入隐私/原生安装/多客户端一致性 | TSF **已有且可用**（`pime-tsf`）；F1 智能修改在 `Programming_IME_SRS_v1.2.md` 的既有范围内；Vercel Router / 可视化**无** |
| **M4** | 语言实现扩展、算法正式认证、组织策略、签名与运维 | 语言矩阵、发布签署与升级兼容 | 语言覆盖在既有 SRS 已有规划；TASE 侧的算法认证/签名/运维体系**无** |

**TASE 的另一套开发次序**（DD §14 `S0～S5`，`IDE1_TASE_Detailed_Design_v1.1.md:783-792`）与 M0～M4 是同一工作的两种切法，S0≈M0、S1≈M1、S2≈M1～M2、S3≈M2、S4≈M3、S5≈M4。其「禁止条件」对本仓库同样成立：**不得为了满足性能目标跳过文档一致性、编译/测试验证、授权审批或版本 CAS**。

---

## 7. 已知落差 / 风险

> 以下均为**如实记录**，未作粉饰，也未在本仓库做任何修补。每条给出证据位置、影响与建议裁定方向，供决策使用。

### G1 — `Document::apply` 在本仓库没有等价入口（**高**）

- **TASE 假设**：`ide-kernel::Document::apply` 是唯一正式写入入口，负责记录撤销（`IDE1_TASE_Requirements_v1.1.md:22`；`IDE1_TASE_Basic_Design_v1.1.md:16`）。
- **本仓库实况**：`pime-core` 没有 `Document` 类型，也没有 `apply` 方法。等价物是 `edit::transaction::Coordinator` 的四步协议（`create_plan` → `prepare` → `begin_apply` → `finish_apply`，`pime-core/src/edit/transaction.rs:227,330,373,427`）。**文本写入由宿主编辑器执行**，`pime-core` 自身不持有 buffer、不持有 undo 栈；`transaction.rs:556` 里的 undo 栈属于测试替身 `FakeDoc`。
- **影响**：TASE 的「一个 Plan 呈现为单一逻辑 undo 事务」（`IDE1_TASE_Detailed_Design_v1.1.md:622`）与「回滚并恢复 buffer dirty/cursor 状态」无法由 `pime-core` 单独保证；多文件 Git 事务（TASE 的 D-05，`IDE1_TASE_Requirements_v1.1.md:232`）更无落点。
- **裁定方向**：(a) 承认 TASE 的编辑内核假设在 IME1.0 不成立，把 `tase-executor` 的 Apply 降级为「生成受约束 diff + 交宿主编辑器单次 `applyEdit`」；(b) 在 `pime-core` 内新增一层真正的 `Document` 抽象（工作量大，且与「core 不持有 buffer」的现状冲突）。**TASE 自己已声明该 PoC 未做**（`IDE1_TASE_Basic_Design_v1.1.md:337`）。

### G2 — 协议面比 TASE 假设的窄（**中高**）

- **TASE 假设**：`ide-shell-protocol` 有 HTTP/WS/stdio 三种接入和 Buffer/Provider trait（`IDE1_TASE_Requirements_v1.1.md:23`），并要求保留 `-32700/-32600/-32601/-32602/-32603`（`IDE1_TASE_Detailed_Design_v1.1.md:246`）。
- **本仓库实况**：有 stdio MCP（`pime-agent/src/mcp.rs`）、有管道承载的 HTTP（`pime-agent/src/serve.rs`，仅 `GET /health` + `POST /`）、**无 WebSocket**、无 Buffer/Provider trait 对应物；标准错误码只见 `-32700/-32600/-32601`（`pime-agent/src/mcp.rs:23,26,29`）。
- **影响**：TASE 的 REST 路由表（`/api/v1/tase/...`）无法直接落地；`tase.*` 方法的对外可达面与设计不同。
- **裁定方向**：(a) 只做 stdio + 单一 POST 端点（最小改动，符合本仓库现状）；(b) 补齐 REST 路由层与缺失错误码；(c) 引入 WebSocket。

### G3 — 无 Web / Tauri / OIDC / 多租户前端（**中高**）

- **TASE 假设**：`web-router`（Next.js App Router）、Vercel Router、OIDC、多租户 RLS、移动 Web 审批（`IDE1_TASE_Basic_Design_v1.1.md:86,292`；DD §2.2）。
- **本仓库实况**：`01_Source/crates` 下**没有 Web 前端 crate**；桌面侧只有 `pime-tray` 与 `pime-setup`。多租户、RLS、签名 URL 在本仓库无对应基础。
- **影响**：TASE 的 M3「Vercel Router / 可视化」与「移动 Web 仅浏览与审批」在本仓库无落点；DD §8.1 的本地链路起点 `Tauri/CLI` 也不成立。
- **裁定方向**：明确 IME1.0 **不做 Web 前端**，TASE 的 M3 Web 部分整体降级为「仅本地 CLI/HTTP/MCP」；或单独立项。

### G4 — `tase-local-ipc` 与 `pime-ipc` 功能重叠，且帧格式不一致（**中**）

- **TASE 假设**：4 字节小端 `u32` 长度前缀 + UTF-8 JSON，默认单帧上限 1 MiB（`IDE1_TASE_Detailed_Design_v1.1.md:851`）。
- **本仓库实况**：`pime-ipc` 是 8 字节头（2 字节 magic `"PI"` + 长度），`CONTROL_FRAME_MAX=64 KiB`、`BULK_FRAME_MAX=2 MiB`、`HANDSHAKE_FRAME_MAX=4 KiB`、硬上限 16 MiB（`pime-ipc/src/frame.rs:17-25`）。
- **影响**：若 Hub 与 Agent 各自实现一侧，必然对不上；重复实现一套管道 = 重复实现一套已经做好的 ACL 与帧。
- **裁定方向**：`tase-local-ipc` 建议**降级为薄适配层**（复用 `pime-ipc` 的 `pipe.rs`/`winsec.rs`/`frame.rs`），只补 TASE 需要而 `pime-ipc` 没有的部分：logon-SID 命名空间、协议能力协商、每连接 session 租约。
- **附带差距**：`pime-ipc` 的 DACL 绑「当前用户 SID」（`winsec.rs:141-142`），TASE 要求绑「**logon SID**」并按 `\\.\pipe\ide1-tase-{logon-sid}` 隔离登录会话；`current_session_id()`（`winsec.rs:206`）目前只被记录、未参与命名空间。

### G5 — `tase-ime-tsf` 与 `pime-tsf` 是「新建」还是「扩展」（**中，未裁定**）

- **TASE 假设**：`tase-ime-tsf` 是 Windows TSF 适配 DLL，职责为 COM 适配、composition、候选 UI、Hub IPC client（`IDE1_TASE_Detailed_Design_v1.1.md:835-836`）。
- **本仓库实况**：`pime-tsf` 已经是一个完整可用的 TSF 输入法（`register.rs`、`controller.rs`、`laser*.rs`、`predictor.rs`、`keymap.rs`、`agent_bridge.rs`），并且已经实现「IME 不含 LLM/网络凭证」这条 TASE 硬约束。
- **影响**：**再建一个 TSF DLL 会与现有 TSF 注册/安装路径冲突**，且候选 UI、composition 两套实现无谓重复。TASE 的 `tase-ime-tsf` 真正新增的只有「Hub IPC client」这一件事。
- **裁定方向**：(a) 把 `tase-ime-tsf` 合并进 `pime-tsf`（新增 `ipc_client.rs` 等模块），**推荐**；(b) 保持独立 crate 但共用 `tase-ime-core` 的候选排序；(c) 独立 DLL（**不推荐**，冲突风险高）。
- **附带差距**：`pime-tsf::agent_bridge` 用的是 `WM_COPYDATA` 窗口消息、方向是「Agent → IME」（`pime-tsf/src/agent_bridge.rs:8,186`）；TASE 要的是「IME → Hub」的 Named Pipe JSON-RPC client，**方向与传输都不同**。

### G6 — 无模板资产体系（**中**）

- **TASE 假设**：`templates/<lang>/<template>/<semver>/` 目录树 + `manifest.yaml` + `params.schema.json` + fragments + fixtures + tests（`IDE1_TASE_Basic_Design_v1.1.md:270-277`；DD §3.1）。
- **本仓库实况**：无模板目录、无 manifest/params schema 概念。既有 SRS 的 F1 智能修改路径走的是 AI 改写，不是模板实例化。
- **影响**：`tase-registry` 整个 crate（digest、许可、审批、生命周期 `DRAFT→…→REVOKED`）在本仓库是从零开始。
- **裁定方向**：要么接受 `tase-registry` 为纯新建并单独排期，要么把 TASE 的「Template」模式降级为「Freeform Patch + 强校验」。

### G7 — 持久化模型完全不同（**中**）

- **TASE 假设**：PostgreSQL（云端，`jobs`/`leases`/`commits`/`audit_events`/`cost_records`/`symbol_index`）+ SQLite WAL（本地，DD §18.1）+ Git Objects/Artifact Store。
- **本仓库实况**：`01_Source` 内 **grep `sqlite` / `rusqlite` 无任何命中**，仓库不引入 SQL 依赖；持久化是 JSON/Markdown（`pime-core/src/settings_io.rs`、`settings.rs`、`settings_migrate.rs`、`pack.rs`）。
- **影响**：`tase-local-store` 若照抄 SQLite WAL DDL，等于引入仓库第一个 SQL 依赖（构建体积、依赖策略、CI 都要改）；`audit_events` 的 append-only 与 `cost_records` 的不可变语义也无现成落点。
- **裁定方向**：(a) `tase-local-store` 选 SQLite（引入新依赖，需评审依赖政策）；(b) 用现有 JSON/Markdown 存储风格实现（要重新设计 append-only 与幂等语义）；(c) 分层——索引/状态用 JSON，审计用追加式日志文件。

### G8 — 位置语义：char vs UTF-16（**中**）

- **TASE 假设**：`ide-kernel` 用 char-based 行列；需 `PositionMapper` 统一 char / UTF-8 byte / LSP UTF-16（`IDE1_TASE_Basic_Design_v1.1.md:259`）。
- **本仓库实况**：编辑偏移是 **UTF-16 code unit**（`pime-core/src/edit/transaction.rs:226` 注释明示；`pime-core/src/scope.rs:120,140` 同口径）。
- **影响**：TASE 的 `PositionMapper` 若照抄「char 为基准」，会与本仓库的编辑路径错位；emoji、日文、CRLF、BOM 场景会直接算错位置。`tase-semantic` 的 AST 侧（syn/Tree-sitter，偏 UTF-8 byte）因此需要一条 **byte → UTF-16** 的换算链。
- **裁定方向**：把 `tase-semantic/src/position.rs` 的基准改为 **UTF-16**，char 与 byte 均为派生视图；同时修正 TASE 文档中「`ide-kernel` 是 char-based」这句事实前提（该前提对本仓库不成立）。

### G9 — `ide-fast` 等仅出现在包清单的 crate 角色不明（**低**）

- **TASE 假设**：`Cargo.toml` 列出 `ide-kernel`、`ide-kernel-core`、`ide-cli`、`ide-shell`、`ide-shell-web`、`ide-shell-desktop`、`ide-shell-protocol`、`editcore`、`ide-fast`（`IDE1_TASE_Requirements_v1.1.md:21`），但正文**从未展开** `ide-fast` 的职责。
- **本仓库实况**：无对应物。
- **影响**：无法判断它是否承载 TASE 依赖的某项能力（例如快速索引或快速通道）。
- **裁定方向**：落地前查证 IDE1.0 侧 `ide-fast` 的实际职责；在此之前**按无对应物处理**，不写入映射。

### G10 — 语义分析能力不可直接复用（**中**）

- **TASE 假设**：`tase-semantic` 用 `syn` + `quote` + `rustfmt` 做结构化 Rust 操作，Tree-sitter 做增量导航与符号探索（`IDE1_TASE_Basic_Design_v1.1.md:263`；DD §4）。
- **本仓库实况**：`pime-core/src/symbols.rs` 是**按 profile 规则的正则抽取**（`extract_symbols(profile, text, max)`，仅扫前 200 KiB），`pime-core/src/analysis.rs` 是正则式上下文分类（`classify_context`、`enclosing_block`），**不是 AST 解析器**。
- **影响**：TASE 的「符号级定位、同名候选 disambiguate、影响面计算」在本仓库**没有可复用基础**；`tase-semantic` 是从零新建，且需要引入 `syn`/`tree-sitter` 依赖。
- **裁定方向**：把 `tase-semantic` 明确列为新建且独立排期；不要把 `pime-core::symbols` 当作它的基础。

### G11 — TASE 的容量与性能数字是「设计目标」，不是本仓库基线（**低，但需注明**）

- **TASE 自述**：「本文件中的性能与容量指标为验收目标，非真实测量数据」（交付说明第 36 行）；DD §23 末尾亦声明「实际能否达到容量、Token 和响应阈值须以实现后的 Windows 环境测试为准」（`IDE1_TASE_Detailed_Design_v1.1.md:1336`）。
- **本仓库实况**：既有文档体系（`Programming_IME_*`）的性能/容量数字同样标注为「暂定验收基线，非实测」。
- **影响**：M2 的「64 连接、16 并行准备」、M3 的「输入隐私 / 原生安装」等验收重点，**不能**在未实测前当作达成。
- **裁定方向**：任何引用 TASE 数字的验收材料，须保留「设计目标 / 未实测」标注。

### G12 — 文档内引用旧文件名（**低，本次已按指示处理**）

- TASE 正文多处引用旧文件名（`01_要件定义.md`、`02_基本设计.md`、`03_详细设计.md`、`04_需求追踪矩阵.xlsx`、`IDE1.0_TASE_完整设计_v1.1.md`），例如 `IDE1_TASE_Basic_Design_v1.1.md:8-9`、`IDE1_TASE_Detailed_Design_v1.1.md:8`、`IDE1_TASE_Basic_Design_v1.1.md:323`。
- 本次按指示**只改了交付说明的「文件」清单**（`IDE1_TASE_Delivery_README_v1.1.md` 第 21-25 行），**正文引用未改**（按「Markdown 内部相对链接/锚点不必修」处理）。
- **影响**：TASE 正文内的这些引用在本仓库路径下**点不开**，读者需回到本文件与 `../README.md` 的对照表。
- **裁定方向**：若需要全量修正，需要一次独立的「文档内引用重写」作业（会破坏与源文件的逐字一致性），请单独决定。

### G13 — 两套文档体系的编号/术语并置（**低，需要口径约定**）

- 本仓库既有体系编号是 `SRS-PIME-001` / `BD-PIME-001` / `DD-PIME-001` / `TDS-PIME-UT|IT|ST-001`；TASE 用的是 `IDE-TASE-BD-001` / `IDE-TASE-DD-001`。
- 两者 FR/NFR 编号**独立且重叠**：`Programming_IME_SRS_v1.2.md` 用 `FR-001～FR-052` / `NFR-101～NFR-601`；TASE 用 `FR-001～FR-064` / `NFR-001～NFR-028`。
- **影响**：跨文档引用 `FR-016` 会产生歧义——TASE 的 FR-016（经 `ide-kernel` 统一编辑接口执行正式更改）与 PIME 的 FR-016 不是同一件事。
- **裁定方向**：约定引用一律带体系前缀（如 `TASE/FR-016`、`PIME/FR-016`），并**不要**把 TASE 的追踪矩阵（`IDE1_TASE_Traceability_v1.1.xlsx`）与 `03_Traceability/Programming_IME_Requirements_Traceability_v1.2.xlsx` 合并或对齐编号。

---

## 8. 一句话结论

- **可以直接复用的**：`pime-ipc` 的管道安全基线、`pime-tsf` 的 TSF 与候选 UI、`pime-agent` 的 MCP/CLI/HTTP 前端、`pime-core` 的计划-令牌-失效事务骨架，以及 TASE 的**依赖方向约束**（contract 在底、executor 不反向依赖协议层）。
- **必须从零新建的**：`tase-contract` / `tase-registry` / `tase-semantic` / `tase-planner` / `tase-executor` / `tase-worker` / `tase-metrics` / `tase-local-hub` / `tase-local-ipc` / `tase-agent-scheduler` / `tase-local-store` / `tase-catalog` / `tase-catalog-mcp` / `tase-ime-core` / `tase-ime-tsf` 共 15 个 crate。
- **必须由决策裁定、不能由实现绕过**的：G1（编辑内核等价性）、G3（是否做 Web 前端）、G5（`tase-ime-tsf` 与 `pime-tsf` 的关系）、G7（持久化选型）、G8（位置基准）、G12（是否全量改文档内引用）。
