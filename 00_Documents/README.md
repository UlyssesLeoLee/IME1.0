# IME1.0 文档索引（`00_Documents/`）

| 属性 | 内容 |
|---|---|
| 适用范围 | 本仓库 `00_Documents/` 整棵子树 |
| 文档体系 | ① **Programming_IME**（本仓库原生体系：SRS / BD / DD / UT·IT·ST / 追踪矩阵）<br>② **IDE1_TASE**（2026-11-05 交付的 TASE v1.1 设计，归档于本仓库） |
| 整理日期 | 2026-11-05 |
| 维护约定 | 本文件只做**索引与说明**；正文以各文件自身内容为准。本文件不替代任何设计文档 |
| 阅读提示 | ② 体系的设计是为**另一个仓库 IDE1.0** 写的，假定存在 `ide-kernel` / `editcore` / `ide-shell-protocol` 等 crate；本仓库**没有**这些 crate。落地前请先读映射说明（见下方「从哪里开始」） |

---

## 从哪里开始

| 你想做的事 | 建议入口 |
|---|---|
| 了解本仓库输入法的需求与基本设计 | `00_Requirements_and_Basic_Design/Programming_IME_SRS_v1.2.md` → `Programming_IME_Basic_Design_v1.0.md` |
| 了解实现级细节 | `01_Detailed_Design/Programming_IME_Detailed_Design_v1.0.md` |
| 了解测试设计（注意：全部未执行） | `02_UT_IT_ST/` 下三份 |
| 查需求 / 测试的追踪矩阵 | `03_Traceability/` 下两份 xlsx |
| 读 TASE v1.1 的完整设计（合并阅读版） | `04_Complete_Design/IDE1_TASE_Complete_Design_v1.1.md` |
| **判断 TASE v1.1 能否落到本仓库、差在哪里** | **`04_Complete_Design/IDE1_TASE_vs_IME1.0_Mapping_v1.1.md`** ← 本次整理的核心交付物 |
| 了解 TASE 交付了什么、作者自述的边界 | `04_Complete_Design/IDE1_TASE_Delivery_README_v1.1.md` |

---

## 文件清单

### `00_Requirements_and_Basic_Design/` — 要件定义与基本设计

| 路径 | 说明 | 版本 | 来源 |
|---|---|---|---|
| `00_Requirements_and_Basic_Design/Programming_IME_SRS_v1.2.md` | 本仓库原生《软件要件定义书》：目标、用户场景、功能/非功能要件（`FR-001～FR-052`、`NFR-101～NFR-601`）、语言覆盖与验收基线 | v1.2 | 既有 |
| `00_Requirements_and_Basic_Design/Programming_IME_Basic_Design_v1.0.md` | 本仓库原生《软件基本设计书》：设计模块清单 `M01～Mxx`、基本设计编号 `BD-01～`、接口/数据/画面基本设计 | v1.0 | 既有 |
| `00_Requirements_and_Basic_Design/Programming_IME_Basic_Design_Traceability_v1.0.xlsx` | 基本设计追踪表，12 个工作表：概要／设计模块／FR设计追踪／NFR设计追踪／画面设计／接口设计／数据字典／例外处理／语言矩阵／E2E场景／PoC门禁／参考资料 | v1.0 | 既有 |
| `00_Requirements_and_Basic_Design/IDE1_TASE_Requirements_v1.1.md` | **TASE 体系**《要件定义书》：TASE 的目标与范围、参与者与约束、FR-001～FR-064、NFR-001～NFR-028、RD-LOCAL / RD-CATALOG / RD-IME 业务要件、未决决策项 | v1.1 | 本次交付（源自 `01_Requirements.md`） |
| `00_Requirements_and_Basic_Design/IDE1_TASE_Basic_Design_v1.1.md` | **TASE 体系**《基本设计书》：双部署拓扑、本机多 Agent 架构（BD-LOCAL）、模块边界与新增 crate、API 总则、数据与事务、模板/UI/安全/容量、模式与算法目录（BD-CATALOG）、IME 边界（BD-IME） | v1.1 | 本次交付（源自 `02_Basic_Design.md`） |

### `01_Detailed_Design/` — 详细设计

| 路径 | 说明 | 版本 | 来源 |
|---|---|---|---|
| `01_Detailed_Design/Programming_IME_Detailed_Design_v1.0.md` | 本仓库原生《软件详细设计书》：内部设计、模块与数据结构、状态机、失败模式与恢复、变更记录 | v1.0 | 既有 |
| `01_Detailed_Design/IDE1_TASE_Detailed_Design_v1.1.md` | **TASE 体系**《详细设计书》：DD-ARCH 工程分包与依赖、Contract/RPC 规范、模板与语义索引、Apply 与 Git CAS、PostgreSQL/SQLite 参考 DDL、Worker 安全、DD-LOCAL 本地多 Agent IPC 与调度、DD-CATALOG 资产目录、DD-IME TSF 与候选、DD-ROADMAP M0～M4 | v1.1 | 本次交付（源自 `03_Detailed_Design.md`） |

### `02_UT_IT_ST/` — 测试设计

| 路径 | 说明 | 版本 | 来源 |
|---|---|---|---|
| `02_UT_IT_ST/Programming_IME_UT_Test_Design_v1.0.md` | 《单元测试设计书》：UT 用例设计（**仅设计，全部未执行**） | v1.0 | 既有 |
| `02_UT_IT_ST/Programming_IME_IT_Test_Design_v1.0.md` | 《集成测试设计书》：IT 场景设计（**仅设计，全部未执行**） | v1.0 | 既有 |
| `02_UT_IT_ST/Programming_IME_ST_Test_Design_v1.0.md` | 《系统测试设计书》：ST 场景设计（**仅设计，全部未执行**） | v1.0 | 既有 |

### `03_Traceability/` — 追踪矩阵

| 路径 | 说明 | 版本 | 来源 |
|---|---|---|---|
| `03_Traceability/Programming_IME_Requirements_Traceability_v1.2.xlsx` | 需求追踪矩阵，7 个工作表：总览／模块清单／功能需求／语言覆盖／非功能需求／关键验收／风险决策 | v1.2 | 既有 |
| `03_Traceability/Programming_IME_Detail_Test_Traceability_v1.0.xlsx` | 详细—测试追踪矩阵，10 个工作表：Dashboard／Requirements／Modules／Interfaces／UT_Cases／IT_Cases／ST_Cases／Fixtures／Gates／Environments | v1.0 | 既有 |
| `03_Traceability/IDE1_TASE_Traceability_v1.1.xlsx` | **TASE 体系**追踪矩阵，7 个工作表：总览／FR需求追踪／NFR追踪／测试场景／设计决策／模式算法目录／共享API清单 | v1.1 | 本次交付（源自 `04_Traceability.xlsx`） |

### `04_Complete_Design/` — 合并阅读版、交付说明与映射（**本次新建目录**）

| 路径 | 说明 | 版本 | 来源 |
|---|---|---|---|
| `04_Complete_Design/IDE1_TASE_Complete_Design_v1.1.md` | **TASE 体系**三合一合并阅读版：要件 + 基本设计 + 详细设计合并为一份，作者建议作为代码生成 Agent 的统一设计输入 | v1.1 | 本次交付（源自 `05_Complete_Design.md`） |
| `04_Complete_Design/IDE1_TASE_Delivery_README_v1.1.md` | **TASE 体系**交付说明：设计范围、文件清单、开发顺序（M0～M4）、架构原则、官方参考 | v1.1 | 本次交付（源自 `README.md`） |
| `04_Complete_Design/IDE1_TASE_vs_IME1.0_Mapping_v1.1.md` | **本次整理的核心交付物**：TASE 每个 crate/模块 → 本仓库落点的三类映射（已有可复用 / 可适配 / 必须新建）、M0→M4 里程碑现状、13 条已知落差与风险 | v1.1 | **本次新写** |
| `04_Complete_Design/README.md` | 本目录的局部说明与文件清单 | v1.1 | **本次新写** |

---

## 本次整理做了什么、没做什么

**做了**：

1. 新建 `04_Complete_Design/`，沿用既有的 `NN_` 编号前缀惯例，把它放在既有编号序列（`00`～`03`）之后。
2. 把 TASE v1.1 的 5 份 Markdown + 1 份 xlsx 逐字搬运到对应目录，统一命名为 `<体系>_<类型>_v<版本>.<扩展名>`，与既有 `Programming_IME_*` 同风格：
   - `IDE1_TASE_Requirements_v1.1.md` / `IDE1_TASE_Basic_Design_v1.1.md` → `00_Requirements_and_Basic_Design/`
   - `IDE1_TASE_Detailed_Design_v1.1.md` → `01_Detailed_Design/`
   - `IDE1_TASE_Traceability_v1.1.xlsx` → `03_Traceability/`
   - `IDE1_TASE_Complete_Design_v1.1.md` / `IDE1_TASE_Delivery_README_v1.1.md` → `04_Complete_Design/`
3. 全部文件保持 **UTF-8 无 BOM + LF 行尾**（Markdown）；xlsx 按二进制原样搬运。
4. 把交付说明里的「文件」清单改成**实际落地的新文件名**（正文其余部分逐字未改）。
5. 写出本索引与映射说明。

**没做**（明确留给决策，见映射说明 §7）：

- 未修改 TASE 的任何正文内容；正文内对旧文件名（`01_要件定义.md` 等）的引用按「内部链接不必修」保留。
- 未修改 `00_Documents/` 以外的任何文件；未执行 `git add` / `commit` / `push`。
- 未对 TASE 与本仓库的落差做任何「修补」或「等价性假设」——全部如实记录在映射说明 §7。
- 未替决策者选择：`tase-ime-tsf` 与 `pime-tsf` 的关系、持久化选型、Web 前端取舍、编辑内核等价性等，均标为待裁定。

---

## 命名与体系约定（两套并存，勿混编号）

| 项 | Programming_IME（本仓库原生） | IDE1_TASE（本次归档） |
|---|---|---|
| 文档编号前缀 | `SRS-PIME-001` / `BD-PIME-001` / `DD-PIME-001` / `TDS-PIME-UT·IT·ST-001` | `IDE-TASE-BD-001` / `IDE-TASE-DD-001` |
| 文件名前缀 | `Programming_IME_*` | `IDE1_TASE_*` |
| FR / NFR 编号 | `FR-001～FR-052`、`NFR-101～NFR-601` | `FR-001～FR-064`、`NFR-001～NFR-028` |
| 追踪矩阵 | `03_Traceability/Programming_IME_*_Traceability_*.xlsx` | `03_Traceability/IDE1_TASE_Traceability_v1.1.xlsx` |

> ⚠️ **两套 FR/NFR 编号独立且存在重叠**。跨文档引用请一律带体系前缀（如 `TASE/FR-016`、`PIME/FR-016`），并**不要**合并或对齐两份追踪矩阵的编号。

---

## 编码与格式约定

| 类型 | 约定 |
|---|---|
| Markdown | UTF-8 **无 BOM**，**LF** 行尾，中文正文；代码路径、标识符、命令、协议方法名保持英文原样 |
| xlsx | 二进制原样搬运，不经文本管道转换 |
| 路径写法 | 仓库内路径使用 `/` 分隔并以 `01_Source/`、`00_Documents/` 起头；Windows 落地时由构建/安装环节处理 |
