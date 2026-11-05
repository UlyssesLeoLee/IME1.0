# IDE1.0 TASE v1.1 — 交付说明

| 属性 | 内容 |
|---|---|
| 设计版本 | v1.1 / 2026-11-05 |
| 交付类型 | 要件定义／基本设计／详细设计＋追踪矩阵 |
| 适用项目 | https://github.com/UlyssesLeoLee/IDE1.0 |
| 工作状态 | 设计终稿（**非已实现/已测试功能**） |
| 编制参考 | 日本 IPA 阶段式软件工程文档方法，不代表 IPA 官方审查或认证 |

## 本次正式设计范围

- 现有 TASE 模板感知语义执行系统：结构化 Plan、语义编辑、预览、文件级提交、版本 CAS、Token/成本度量。
- **同机多 Agent：** 每 Windows 登录会话一个 Local Agent Hub、Named Pipe IPC、独立 Agent Session、权限隔离、共享不可变索引、资源配额、公平队列、写入仲裁与故障恢复。
- **模式与算法标准库：** GoF 23、现代工程模式 20、经典算法 50，合计 93 个目录资产。目录属于“设计资产清单”，**不是 93 个已认证实现**。
- **输入法桥接：** Windows TSF 适配器仅处理人类文字输入与候选交互；Agent 直接以 MCP/JSON-RPC 调用目录和 TASE 执行器。
- 测试与追踪：共 64 个功能要件、28 个非功能要件、49 个**计划**测试场景；全条目应遵循需求→设计→测试的可追溯性。

## 文件

1. `../00_Requirements_and_Basic_Design/IDE1_TASE_Requirements_v1.1.md` — 目标、用户场景、FR/NFR、业务安全和验收。
2. `../00_Requirements_and_Basic_Design/IDE1_TASE_Basic_Design_v1.1.md` — 本地/云部署、模块切分、接口边界、存储、调度、资产策略与 TSF 交互。
3. `../01_Detailed_Design/IDE1_TASE_Detailed_Design_v1.1.md` — IPC 帧/RPC、Agent 生命周期、数据结构、伪代码、SQLite DDL、资产 Manifest、MCP Schema、并发和 IME 测试。
4. `../03_Traceability/IDE1_TASE_Traceability_v1.1.xlsx` — 7 个工作表：总览、FR、NFR、测试、ADR、模式算法目录、共享 API 清单。
5. `IDE1_TASE_Complete_Design_v1.1.md` — 合并阅读版，优先作为代码生成 Agent 的统一设计输入。

## 开发顺序

M0 契约与授权 → M1 本地 Hub/单 Agent 核心 → M2 多 Agent/目录/工具 → M3 TSF/跨端/远程 → M4 语言实现与审核扩展。

## 架构原则

1. 目录知识卡与可执行语言实现分开审核；未认证实现不能自动应用到正式源码。
2. 多 Agent 能并发准备，但同文件正式写入及 Git ref 推进必须冲突检测和线性化。
3. 默认本地离线运行；联网模型/云端能力仅按显式权限执行。
4. 本文件中的性能与容量指标为验收目标，非真实测量数据。

## 官方参考

- Microsoft Windows TSF IME requirements: https://learn.microsoft.com/en-us/windows/apps/develop/input/input-method-editor-requirements
- Microsoft Named Pipe Security: https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-security-and-access-rights
- MCP tool schema: https://github.com/modelcontextprotocol/modelcontextprotocol/blob/main/docs/specification/2026-07-28/server/tools.mdx
- Vercel Functions limits: https://vercel.com/docs/functions/limitations
