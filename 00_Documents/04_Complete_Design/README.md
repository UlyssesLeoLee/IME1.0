# `04_Complete_Design/` — 合并阅读版、交付说明与映射说明

| 属性 | 内容 |
|---|---|
| 目录用途 | 存放 **IDE1_TASE v1.1**（2026-11-05 交付）的合并阅读版、交付说明，以及本次整理产出的 **TASE ↔ IME1.0 映射说明** |
| 目录来源 | 本次新建（2026-11-05），编号 `04` 沿用 `00_Documents/` 既有 `NN_` 前缀惯例，排在既有 `00`～`03` 之后 |
| 体系 | IDE1_TASE（**非**本仓库原生体系，详见下方说明） |

---

## 文件清单

| 文件 | 说明 | 版本 | 来源 |
|---|---|---|---|
| `IDE1_TASE_Complete_Design_v1.1.md` | TASE v1.1 三合一合并阅读版：要件定义书 + 基本设计书 + 详细设计书合并为一份，作者建议作为代码生成 Agent 的统一设计输入 | v1.1 | 本次交付（源自 `05_Complete_Design.md`，逐字搬运） |
| `IDE1_TASE_Delivery_README_v1.1.md` | TASE v1.1 交付说明：设计范围、文件清单、开发顺序（M0～M4）、架构原则、官方参考链接 | v1.1 | 本次交付（源自 `README.md`，逐字搬运，仅「文件」清单改为实际落地文件名） |
| `IDE1_TASE_vs_IME1.0_Mapping_v1.1.md` | **本次整理的核心交付物**：TASE 每个 crate/模块 → 本仓库（IME1.0）落点的三类映射（已有可复用 / 本仓库没有但可适配 / 必须新建）、非 crate 交付物落点、M0→M4 里程碑现状、13 条已知落差与风险 | v1.1 | **本次新写** |
| `README.md` | 本目录的局部说明 | v1.1 | **本次新写** |

---

## 重要前提：这套设计不是为本仓库写的

TASE v1.1 的适用项目是 **IDE1.0**（<https://github.com/UlyssesLeoLee/IDE1.0>），其设计假定存在 `ide-kernel`、`editcore`、`ide-shell-protocol` 等 crate。

**本仓库（IME1.0）没有这些 crate。** 本仓库的对应物是：

| TASE 假定存在 | 本仓库实际有 |
|---|---|
| `ide-kernel` / `editcore`（唯一文本修改内核、undo/redo、字符位置编辑） | `pime-core`（`edit/transaction.rs`、`textrange.rs`、`session.rs`）——**但写入与 undo 归宿主编辑器，`pime-core` 不持有 buffer** |
| `ide-shell-protocol`（JSON-RPC / HTTP / WS / stdio） | `pime-agent`（`cli.rs`、`serve.rs`、`mcp.rs`）+ `pime-core::protocol`——**有 stdio 与 HTTP，无 WebSocket** |
| 命名管道 + ACL | `pime-ipc`（SDDL DACL、`PIPE_REJECT_REMOTE_CLIENTS`、分帧/握手）——**与 TASE 的帧格式不一致** |
| TSF 输入法 | `pime-tsf`（注册、控制器、激光候选窗、预测器、快捷键）——**已是完整输入法，不是薄适配层** |

因此**不能照单施工**。落地前请先读 `IDE1_TASE_vs_IME1.0_Mapping_v1.1.md`，尤其：

- **§2** 已有可复用的四个 crate（各含「可直接沿用」与「必须先解决的不等价」两张表）；
- **§3** TASE 正文中直接点名既有 crate 的 **18 处**位置逐条对照；
- **§4** 必须新建的 **15 个** TASE crate（职责一句话 + 依赖谁 + 落在 `01_Source/crates/<name>/` 的哪个文件）；
- **§6** M0→M4 里程碑现状（验收重点照抄 TASE 原文，第三列为本仓库现状）；
- **§7** **13 条已知落差与风险**，其中 6 条需要决策裁定：G1 编辑内核等价性、G3 是否做 Web 前端、G5 `tase-ime-tsf` 与 `pime-tsf` 的关系、G7 持久化选型、G8 位置基准（char vs UTF-16）、G12 是否全量改文档内引用。

---

## 本目录内文件的编码

Markdown 统一 **UTF-8 无 BOM + LF 行尾**；中文正文，代码路径、标识符、协议方法名保持英文原样。

TASE 源文件位于 `D:\tmp-pime-wiz\ide-docs\`，**只读**，本次整理未修改其中任何文件。
