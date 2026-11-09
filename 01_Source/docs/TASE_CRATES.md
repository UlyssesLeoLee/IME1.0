# TASE 在本仓库的落点（15 个 crate）

> 这份文档说清三件事：**TASE 是什么**、**十五个 crate 各自负责什么**、**它们怎么接在一起**。
> 设计的原文在 `00_Documents/04_Complete_Design/`（要件定义书 / 基本设计书 / 详细设计书），
> **本仓库的对照稿**是同目录的 `IDE1_TASE_vs_IME1.0_Mapping_v1.1.md`（每个 crate 的落点、13 条已知落差 G1~G13）。
> 设计与实现之间还没定的事，都记在 `02_Verification/DEFECT_SUBMISSION_2026-11-07.md`（**缺陷提交单**）里。

## 一句话

TASE（Template-Aware Semantic Execution，设计书 §2）是「模板感知的语义执行」：把一次改动拆成
**意图 → 计划 → 预览 → 应用 → 提交**，每一步都可审、可回退、可被多个 Agent 安全地并发使用。
IME1.0 不做 IDE，本仓库把 TASE 落成 **15 个 Rust crate**：它们是输入法的"大脑侧"能力——
AI 客户端（MCP / CLI / HTTP）通过它们拿到候选、目录与执行能力，输入法本体（TSF DLL）负责界面与上屏。

## 十五个 crate

| crate | 负责什么 | 设计依据 | 依赖 |
|---|---|---|---|
| `tase-contract` | 契约层：DTO、ID 规则、12 个冻结错误码、协议版本 | DD §1.2、RD §4 | `pime-core` |
| `tase-registry` | 资产注册表：manifest 解析、摘要校验、别名检索、策略 | DD §17.1 | `tase-contract` |
| `tase-planner` | 计划器：意图路由、上下文裁剪、计划校验、`plan_digest` | DD §2.3 | `tase-contract` `tase-registry` |
| `tase-metrics` | 度量：token、成本、延迟、ACI 验收指标、追踪 | DD §9 | `tase-contract` |
| `tase-local-ipc` | 本地传输：命名管道、长度前缀分帧、连接管理、ACL | DD §17.1 / §17.3 | `tase-contract` |
| `tase-local-store` | 本地持久化：SQLite WAL、版本化迁移、崩溃对账、快照 | DD §18 | `tase-contract` |
| `tase-semantic` | 语义层：符号索引、引用与影响面、UTF-16 基准的位置换算 | DD §2.7、RD §14 | `tase-contract` |
| `tase-catalog` | 标准资产目录：93 张知识卡的索引、推荐打分、治理与**认证状态机** | DD §19、RD §14.4 | `tase-contract` |
| `tase-catalog-mcp` | 目录的 MCP 投影：`catalog.search/recommend/instantiate/describe` | DD §19 | `tase-contract` `tase-catalog` |
| `tase-ime-core` | 输入法侧的纯逻辑：候选模型、确定性排序、上下文隐私策略 | DD-IME §20.2~§20.4 | `pime-core` |
| `tase-local-hub` | 常驻 Hub：单例、鉴权、会话与授权、工作区、事件流 | DD §17.1 / §17.2 | `tase-local-ipc` `tase-agent-scheduler` `tase-local-store` `tase-catalog` |
| `tase-agent-scheduler` | 每 Agent 公平队列（加权轮转）、资源配额、租约与 fencing、预算、事件分发 | DD §17.5 | `tase-contract` |
| `tase-executor` | 执行器：Preview / Apply / Rebase / 冲突判定 / 工作区快照 / **Git CAS 提交** / 撤销 | DD §8 | `tase-contract` `tase-registry` `tase-planner` `tase-semantic` `pime-core` |
| `tase-worker` | 作业执行：队列消费者、租约与 fencing、沙箱执行、编译/测试验证、故障恢复 | DD §14 | `tase-executor` `tase-metrics` `tase-local-store` |
| `tase-impls` | **目录里五十个算法的 Rust 实现**（每个都有测试，登记为 `TESTED`） | DD §19.1 | 无（只用标准库） |

## 依赖方向（不可反转）

```
tase-contract  ←  registry / semantic / planner / metrics / ipc / store / catalog
      ↓
   tase-executor  ←── pime-core（编辑适配，不反向依赖）
      ↓
  tase-worker   |   tase-local-hub
      ↓
  local-ipc / agent-scheduler / local-store / catalog
      ↓
  catalog-mcp / ime-core ──→ pime-tsf（TSF 适配层，本仓库不单独建 tase-ime-tsf，见映射 §7-G5）
```

**为什么每个 crate 都有自己的 `Cargo.lock`、都不进主 workspace**：主 workspace 的共享 target 目录会在
`windows` crate 换 feature 集时整包重编，峰值内存能把本机（16 GB 内存 + 固定 8 GB 页面文件）压垮。
独立 crate 各自一个 target 目录，是这条机器上唯一跑得动的姿势（D-50）。

## 目录与实现的关系（认证状态机）

`01_Source/assets/tase-catalog/` 里是 **93 张知识卡**（GoF 23 + 工程模式 20 + 算法 50）：
每张卡说清「什么时候用、什么时候**不**用、代价多少、替代方案是什么」。

**卡是文档，实现是代码**，两者由两条轴分开记（RD §14.4）：

| 轴 | 状态 |
|---|---|
| 知识卡 | `DOCUMENTED` / `REVIEWED` |
| 实现 | `IMPLEMENTED` / `TESTED` / `APPROVED` / `DEPRECATED` / `REVOKED` |

本轮落地：`tase-impls` 实现了目录里**全部五十个算法**，每个实现的登记条目写进对应资产的 `manifest.json`：

* `path`：实现所在的源文件；
* `sha256`：该文件的**真实摘要**（按 LF 归一化，与 `MANIFEST_SHA256.txt` 同一把尺子）——设计书禁止占位摘要；
* `parameters_schema`：实现的函数签名；
* `tests_manifest`：`assets/tase-catalog/implementations/<资产 id>.json`，列出**站得住的测试名**；
* `status`：**`TESTED`**（有具名测试）——**不是 `APPROVED`**：那需要一个人看过代码并签字，那是你的裁定（D-61）。

登记与核对是两支脚本：

```powershell
node scripts/collect-implementations.js          # 把实现写回目录（带真实摘要与测试清单）
node scripts/collect-implementations.js --check  # 只报差异
node scripts/validate-tase-catalog.js            # 校验 93 张卡 + 实现条目的字段、摘要与测试清单
```

校验器现在会**逐条核对摘要**：实现指向的文件不在、摘要对不上、声明 `TESTED` 却没有具名测试，一律红。

## 自己跑一遍

```powershell
# 一个 crate（每个 crate 有自己的 target 目录，别用主 workspace 的）
$env:PATH="C:\Users\seiko\.cargo\bin;C:\msys64\mingw64\bin;"+$env:PATH
$env:RUSTUP_TOOLCHAIN="stable-x86_64-pc-windows-gnu"
$env:CARGO_TARGET_DIR="C:\cargo-target-ime\tase-executor"
$env:CARGO_BUILD_JOBS="1"; $env:CARGO_PROFILE_DEV_DEBUG="0"; $env:CARGO_INCREMENTAL="0"
cd 01_Source\crates\tase-executor; cargo test; cargo clippy --all-targets -- -D warnings

# 全部十四个 TASE crate（门禁的那一步）
node 01_Source\tests\run-all.js --tase
```

## 还没定的事

设计书与本仓库之间的落差**没有**被悄悄抹平：13 条 G1~G13 在对照稿 §7，
59 条待裁定的缺陷与取舍在 `02_Verification/DEFECT_SUBMISSION_2026-11-07.md`。
其中三条会挡住后面的活，需要你给一句话：

1. **D-53**：`tase-local-store` 用 SQLite（设计书口径、许可干净、破了零依赖）还是改成零依赖追加式？
2. **D-51**：要不要立项引 AST 依赖（`syn` / `tree-sitter`）？它决定 `tase-semantic` 是行级扫描还是完整 AST。
3. **D-42**：面板入口要不要加快捷键？F1 在第十三轮已有归属，你给一个键。
