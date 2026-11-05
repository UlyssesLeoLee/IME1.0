# IDE1.0 — TASE 模板感知语义执行系统：要件定义书

| 属性 | 内容 |
|---|---|
| 文档编号 | IDE-TASE-RD-001 |
| 版次 | 1.1 / 2026-11-05 |
| 状态 | 设计提案（非现有功能声明） |
| 产品 | IDE1.0（Rust 内核 + TUI / Web / Tauri） |
| 适用范围 | TASE + 同机 Local Agent Hub + 共享模式／算法资产目录 + Windows 程序员输入法桥接 |
| 工程参考 | 日本 IPA 的要件定义→基本设计→详细设计→测试/验收追踪方法；**并非 IPA 官方认证模板** |
| 相关文档 | `02_基本设计.md`、`03_详细设计.md`、`04_需求追踪矩阵.xlsx` |

## 1. 文档目的、背景与现状

### 1.1 业务问题

一般 AI IDE 的生成路径是“模型理解→模型连续输出源代码→编辑器写入”。固定样板代码不断由模型输出，带来不必要的输出 Token、等待时间，以及多 Agent 对同一文件重复生成/覆盖的风险。IDE1.0 要把模型的产物从“完整源代码”改为优先输出“受约束的操作计划”，随后由可审计的 Rust 引擎执行模板展开或结构化修改。

### 1.2 经仓库核对的既有能力（2026-11-05，默认分支内容快照）

- 工作区 `Cargo.toml` 的包版本是 **0.3.9**，已列出 `ide-kernel`、`ide-kernel-core`、`ide-cli`、`ide-shell`、`ide-shell-web`、`ide-shell-desktop`、`ide-shell-protocol`、`editcore` 与 `ide-fast`。
- `ide-kernel` 已以 `Document`/`Session` 聚合 `editcore`，声明编辑必须通过统一 `Document::apply` 记录撤销；位置语义采用字符计数。
- `ide-shell-protocol` 已有 JSON-RPC 2.0 风格的 HTTP/WS/stdio 接入、Buffer / Provider trait 等能力；新接口**添加命名空间而不是替换旧接口**。
- 项目声明 AGPL-3.0-only；需要做依赖与生成物许可审查，不能假设所有模板都自动可商用或免合规。
- README 的若干阶段统计与现有 workspace 文件不同步。因此本书以核对过的源文件作为**扩展切入点**，不将 TASE 描述为已经实现。

源：<https://github.com/UlyssesLeoLee/IDE1.0>；重点参照 `Cargo.toml`、`crates/ide-kernel/src/lib.rs`、`crates/ide-shell-protocol/src/lib.rs`、`crates/editcore/src/lib.rs`。

### 1.3 用语

| 术语 | 定义 |
|---|---|
| TASE | Template-Aware Semantic Execution，模板感知语义执行 |
| Template | 经过审核、版本锁定、含参数模式及测试夹具的可复用代码生成资产 |
| Semantic Unit | 具稳定符号标识与来源范围的语义工作单元，如函数、类型、方法、接口、测试用例 |
| Plan | 模型或客户端提交的声明式、无副作用操作意图 |
| Preview | 在不可变快照上生成的候选变更及验证报告，不写正式工作区 |
| Apply | 基于版本前置条件的原子修改/提交 |
| Worker | 隔离环境内执行模板、AST 编辑、格式化、测试的计算进程 |
| Router | 身份认证、租户/项目路由、调度、限流、作业查询的 API 层 |
| ACI | IDE1.0 已接入的结构化断言 / 测试报告约定 |
| CAS | Compare-And-Swap，比对并交换，用于防止过期写入 |

## 2. 目标与范围

### 2.1 目标

- G-01：对于适用预置模板的任务，减少模型重复输出固定源码，提高响应效率。
- G-02：实现可预测、可审计、可回放且可撤销的代码编辑操作。
- G-03：将 Agent 协作单位从“整文件写入”升级为“符号级规划 + 受控文件写入”。
- G-04：以既有 Rust 编辑内核为唯一的文档变更入口，复用 TUI/Web/Tauri。
- G-05：提供机器可发现、模型易调用的轻量 API，支持 Vercel Router 与可扩缩 Worker。
- G-06：对性能、价格、可靠性与冲突率进行实测，避免宣称固定的节省百分比。
- G-07：在同一台电脑上供多个相互独立的 Agent 进程并发使用，不共享可变编辑状态、不覆盖彼此修改。
- G-08：把主要设计模式与经典算法定义为统一可版本化的知识卡、代码模板、验证用例和可调用工具，并与程序员输入法共用。
- G-09：对人类输入法与 Agent API 统一标准资产、分离权限和交互；Agent 不依赖模拟键盘输入。

### 2.2 本期包括

模板注册、发现与锁版；计划校验；语义符号索引；模板参数验证；预览与差异；原子应用；并发冲突检测；重试/幂等；回滚；隔离执行；任务队列/租约；真实 Token 成本统计；安全与审计；Web/Tauri/TUI 调用桥接；Rust/TypeScript 第一批语法支持。

### 2.3 本期不包括

训练或微调基础模型、提供通用云端 IDE 容器租赁服务、承诺无限量实际并发、自动接受未经审核的互联网上传模板、未经批准自动合并受保护分支、将复杂业务算法强制模板化、替代 Git 本身或直接修改用户第三方仓库托管平台的安全策略。Python/Go/Java 等新增解析适配器列入后续阶段。Windows 程序员输入法的 TSF 集成纳入本设计，其最终系统级安装与跨应用兼容需要专项测试。

## 3. 参与者、环境与约束

| 参与者 | 责任 | 权限边界 |
|---|---|---|
| 人类开发者 | 选择模板、查看预览、审核高风险变更、恢复版本 | 项目角色授权 |
| AI Agent | 调用 `tase.*`，生成结构化计划，读取有限上下文 | 仅授予 scope 和能力令牌；不可直接绕过 Apply |
| 模板维护者 | 创建、测试、签名/批准模板版本 | 发布权限独立于使用权限 |
| Router | 认证、鉴权、路由、限流、作业建档 | 不执行任意未隔离命令 |
| Rust Worker | AST / 模板 / Git / 验证 / 结果归档 | 最小权限，仓库快照隔离 |
| 审计者 | 查阅日志、成本、失败、回滚证据 | 脱敏只读 |

平台约束：可部署在 Vercel 的 Web/Router；长任务由队列调度并在受控环境运行。Vercel Functions 是按请求调用且存在运行时限制，不能把无状态函数实例的本地文件系统视为共享、永久的工作区。可选 Vercel Queues/Workflow，但语义最终取决于系统自己的事务与作业库，须应对至少一次投递。官方参考：<https://vercel.com/docs/functions>、<https://vercel.com/docs/queues/concepts>。

## 4. 用例与业务流程

| 用例 | 发起方 | 触发 | 完成条件 |
|---|---|---|---|
| UC-01 | Agent | 需要实现 CRUD | 查找并锁定模板版本，拿到参数 Schema |
| UC-02 | Agent | 需要实施变更 | 获得结构化计划和预计影响范围 |
| UC-03 | 用户/Agent | 审核计划 | 看到差异、风险、成本估计和验证结果 |
| UC-04 | Agent | 提交申请 | 满足版本前置条件后一次性生效 |
| UC-05 | 多 Agent | 同时编辑单文件不同符号 | 可并发规划，依序安全提交，不丢失修改 |
| UC-06 | Agent | 修改冲突 | 收到明确冲突对象及重新计划建议，不静默覆盖 |
| UC-07 | 用户 | 想恢复 | 依据提交证据生成反向变更并再次校验 |
| UC-08 | 审计者 | 检查经济性 | 比对标准模式与 TASE 模式的成本、时间和失败率 |
| UC-09 | 用户 | 模板不匹配 | 进入 AST 编辑或自由生成模式，并留下原因 |

新增主流程：`Agent 注册 → 会话隔离 → 资产发现 → 参数/约束校验 → 计划 → Preview → 排队与仲裁 → Approval Gate（按需）→ Apply → Validation → Commit → Metrics`。人类 IME 交互走候选选择→确认插入路径；Agent 直接调用工具，无键盘焦点步骤。

主流程：`Discovery → Planning → Preview → Approval Gate（按需）→ Apply → Validation → Commit → Metrics`。失败需停在可恢复、可诊断状态，不能宣称成功。

## 5. 功能要件（FR）

**优先级**：P0 第一版必须；P1 第一版后优先；P2 扩展。**验收标识**：所有条目对应 Excel 追踪矩阵的 TC- 编号。

| ID | 优先级 | 要件 / 可验收行为 |
|---|---|---|
| FR-001 | P0 | 允许模板按 `namespace.name@semver` 注册，每版为不可变内容哈希。 |
| FR-002 | P0 | 按语言、用途、框架、license、能力标签和版本约束搜索模板。 |
| FR-003 | P0 | 每个模板具有机器可读参数 JSON Schema、默认值、必需项、输出路径策略与示例。 |
| FR-004 | P0 | 仅在参数 Schema 校验通过、许可策略允许时实例化；失败无任何工作区副作用。 |
| FR-005 | P0 | 计划必须通过统一 Schema 验证并声明目标仓库、基线版本、预计受影响符号与操作类型。 |
| FR-006 | P0 | 支持 `instantiate_template`、`replace_symbol`、`insert_sibling`、`delete_symbol`、`freeform_patch` 五类操作。 |
| FR-007 | P0 | 执行器根据任务特征推荐 Template → Semantic → Freeform 三档策略，允许客户端显式选择。 |
| FR-008 | P0 | Preview 在快照上执行，返回 diff、文件清单、潜在影响和验证摘要，不修改正式分支。 |
| FR-009 | P0 | Apply 必须验证 `base_commit` 及各文件/符号前置条件，禁止过期内容静默覆盖。 |
| FR-010 | P0 | 对同一物理文件实施互斥提交或等效线性化机制，允许不同符号并行准备。 |
| FR-011 | P0 | 不同符号编辑如触及共享导入、类型/接口或格式化范围，必须计算实际写入集合并检测重叠。 |
| FR-012 | P0 | 不能安全重基的并发修改返回 Conflict 及结构化原因；不得猜测合并。 |
| FR-013 | P0 | 根据 `Idempotency-Key` 和计划摘要去重；队列重复投递不重复写文件或提交。 |
| FR-014 | P0 | 对同一作业保留明确状态机、重试次数、租约、心跳与最终结果。 |
| FR-015 | P0 | 对每次 Apply 留存审计事件、变更哈希、版本、Agent 标识、执行工件与验证证据。 |
| FR-016 | P0 | 通过 `ide-kernel` 的统一编辑接口执行正式文档更改，保持 undo/redo 语义；Git 提交与内存会话同步。 |
| FR-017 | P0 | 通过名称空间化 JSON-RPC 暴露模板查询、计划、预览、应用和状态查询；维持现有协议兼容。 |
| FR-018 | P0 | Web、Tauri、TUI 对同一计划产生相同执行结果，禁止三套独立编辑引擎。 |
| FR-019 | P0 | 提供 Rust 的语义索引、符号定位和受控替换；语法不可靠时阻止语义 Apply。 |
| FR-020 | P0 | 提供格式化、语法/编译检查，并可配置测试门禁；未通过时禁止自动发布/提交。 |
| FR-021 | P0 | 路径规范化、工作区边界、模板注入防护、进程/网络/磁盘配额及密钥隔离。 |
| FR-022 | P0 | 租户、仓库、分支、Agent、操作权限必须在 Router 和 Worker 双重验证。 |
| FR-023 | P0 | 记录每次模型调用 input/output/cache/reasoning（若供应商提供）、工具调用、重试、价格快照与总耗时。 |
| FR-024 | P0 | 提供成本/性能基准测试模式，与传统全代码输出在同样任务集上比对。 |
| FR-025 | P0 | 输出可供现有 ACI emitter 消费的 UT/IT/ST/E2E 验证结论。 |
| FR-026 | P1 | 批量模板/语义操作形成 DAG，按依赖顺序调度并发现循环。 |
| FR-027 | P1 | 对 Template Registry 提供审核流、签名、撤销和锁版策略。 |
| FR-028 | P1 | 支持 TypeScript/TSX 的语义索引/修改；失败时安全降级。 |
| FR-029 | P1 | 提供敏感路径/受保护分支/跨模块 API 修改的人工批准门禁。 |
| FR-030 | P1 | 对撤销操作生成带版本前置条件的反向补丁，防止误回滚其他 Agent 修改。 |
| FR-031 | P1 | Agent 获取上下文时按语义单元裁剪，仅返回完成任务所必需的代码及摘要。 |
| FR-032 | P1 | 提供 Web 控制台展示计划、diff、并发冲突、作业队列、成本与模板来源。 |
| FR-033 | P1 | 提供公开 OpenAPI/JSON Schema 工件及 Rust/TypeScript 轻量客户端。 |
| FR-034 | P1 | 仓库内容和模型输入可按策略完全留在客户自托管 Worker；不得默认送至第三方模型。 |
| FR-035 | P2 | 可导入组织内部批准模板包，支持供应链签名及 SPDX/SBOM。 |
| FR-036 | P2 | 支持 Python/Go/Java 等语义适配器的插件接入。 |
| FR-037 | P0 | 同一 Windows 用户会话的 Agent 连接单一 Local Agent Hub；同时支持多个进程独立连接，不要求每 Agent 启动独立编辑器。 |
| FR-038 | P0 | Agent 注册返回唯一身份、会话标识、能力范围、最小权限与心跳租约；断线不自动继承其他 Agent 权限。 |
| FR-039 | P0 | 为每 Agent 隔离临时 Plan、Preview、上下文、执行工件及撤销历史；仅共享授权的不可变模板、索引与快照。 |
| FR-040 | P0 | 本地跨进程 IPC 支持多个连接、背压、异步任务、事件订阅与取消；Windows 默认使用受 ACL 保护的 Named Pipe。 |
| FR-041 | P0 | 按 Agent/项目/操作分类实施公平调度、限流、CPU/内存/编译并发配额，避免单个 Agent 饿死其他作业。 |
| FR-042 | P0 | 本地文件、未保存 Buffer、Git 分支采用统一版本向量/基线前置条件与提交协调器，避免跨进程丢失更新。 |
| FR-043 | P0 | Local Hub 崩溃重启应恢复持久作业、清理过期租约、隔离孤儿沙箱，避免重复提交。 |
| FR-044 | P0 | 支持同一项目多个独立 Agent 同时执行只读查询和模板/算法检索，写入操作必须经过能力鉴权与审计。 |
| FR-045 | P0 | 共享标准资产目录包含 GoF 23 种设计模式及定义范围内的主流工程模式，具唯一 ID、版本与适用性描述。 |
| FR-046 | P0 | 标准算法目录覆盖排序、查找、图、字符串、动态规划、数据结构、并发与数学等经典类别，并提供复杂度及前置条件。 |
| FR-047 | P0 | 模式/算法资产同时具有知识卡、参数模式、语言实现引用、复杂度、正确性用例、反例与来源许可。 |
| FR-048 | P0 | Agent 可用稳定 API/MCP 工具搜索、比较、解释、生成和验证模式或算法；无需模拟输入法按键。 |
| FR-049 | P0 | 所有模式/算法调用必须按语言、版本、平台、规模、排序稳定性、线程安全等约束选择资产，不符合要求时明确拒绝。 |
| FR-050 | P0 | 允许通过 IME / IDE 补全候选访问同一资产目录，候选插入需用户确认，不得在密码/安全输入区域收集上下文。 |
| FR-051 | P0 | 资产展开代码必须符合语言适配器和类型/编译检查，不应把知识说明误当作可运行代码模板。 |
| FR-052 | P0 | 模式/算法版本不可变；支持安全公告、弃用、回滚和语言实现的独立审核版本。 |
| FR-053 | P0 | 本地 Agent 对本地工程的默认执行完全离线可用，联网模型及云 Router 为显式可选项。 |
| FR-054 | P0 | 采用最小上下文捕获与本地 IPC 安全隔离，跨进程调用不得泄露其他 Agent prompt、API Key 或源码片段。 |
| FR-055 | P1 | 编程输入法提供 Rust/Python/TypeScript/PowerShell/C/汇编/机器语言类别，能力以适配器等级对外标识。 |
| FR-056 | P1 | Windows 输入法使用 TSF Text Input Processor 适配层，TSF DLL 不包含大模型/执行器；通过安全 IPC 访问 Local Hub。 |
| FR-057 | P1 | 提供可配置 F1 触发整体修改建议/插入候选、输入字符渐进候选、语言自动识别与手动选择；优先遵守宿主快捷键。 |
| FR-058 | P1 | 输入法与 IDE 统一候选排序、来源与风险提示；显示推荐理由、复杂度、适用场景和替代方案。 |
| FR-059 | P1 | 模式/算法可按项目策略固定推荐规范，例如优先标准库、禁用自实现加密或不允许递归深度超限方案。 |
| FR-060 | P1 | 多 Agent 支持协作任务 DAG、子 Agent 身份继承范围限制、资源预算和取消传播。 |
| FR-061 | P1 | 同机可同时服务多个 IDE / shell / IME 客户端，事件广播按项目/会话订阅过滤并有顺序标记。 |
| FR-062 | P1 | 提供资产版本选择与审批界面，并生成模板/算法的 UT/IT/ST/E2E 测试骨架，保持测试可追溯。 |
| FR-063 | P1 | 共享 Local Hub 提供只读运维指标：连接数、队列延迟、活跃 Agent、冲突、缓存命中、Token/CPU/内存。 |
| FR-064 | P2 | 未来支持局域网受控 Hub 集群，但默认无网络监听；跨 Windows 登录用户必须经独立身份与权限审核。 |

## 6. 非功能要件（NFR）与量化验收

| ID | 要件 | 验收定义 / 说明 |
|---|---|---|
| NFR-001 | 正确性 | 在固定 200 条回归样例中，**零静默数据覆盖、零越权提交**；冲突必须显式标记。 |
| NFR-002 | 幂等性 | 同 `Idempotency-Key` + 同请求摘要重发 100 次，最终只有一个成功提交；同 Key 不同摘要返回错误。 |
| NFR-003 | 原子性 | 每文件正式写入原子可见；多文件任务只有经 Git tree / 事务性提交成功才标记 COMMITTED。 |
| NFR-004 | 可用性 | Router 月可用性目标 ≥99.9%，Worker 失败可恢复；这是设计目标，不是已达成 SLA。 |
| NFR-005 | 审计 | 所有正式修改 100% 保留计划版本、请求标识、模型/模板版本、关联提交及校验结果。 |
| NFR-006 | 性能 | 在“有适用模板”的独立基准任务集上，相比全代码输出模式，输出 Token 中位数减少 ≥70% 为初期目标；**不是所有任务承诺**。 |
| NFR-007 | 响应 | `template.search`/`code.plan` 不含 LLM 耗时的服务端 p95 目标 ≤300ms / ≤1s，基于规定数据集验收。 |
| NFR-008 | 并发 | 支持 ≥1,000 个逻辑 Agent 身份与排队任务；实际并行 Worker 数由租户配额/资源决定，初期基准 32 个并发准备、对同文件线性化提交。 |
| NFR-009 | 跨平台 | Windows/Linux 的路径、Unicode、CRLF/LF 和结果 hash 验证一致（排除显式平台依赖模板）。 |
| NFR-010 | 安全 | 未审核模板禁止带副作用脚本；默认禁止未知出站网络、宿主目录访问和秘密回传。 |
| NFR-011 | 兼容 | 不破坏 `ide-shell-protocol` 现有方法及 `ide-kernel` 文档变更语义。 |
| NFR-012 | 追踪 | 每项 P0 需求至少映射 1 个自动化测试和 1 条设计定位；关键链路提供 E2E。 |
| NFR-013 | 可运营性 | 记录队列深度、租约超时率、冲突率、成功率、验证耗时、Token/任务、费用/成功任务。 |
| NFR-014 | 可恢复 | Worker 任意阶段中断后可判定 NOT_APPLIED/APPLIED/COMMITTED，不产生不明状态的成功响应。 |
| NFR-015 | 成本 | 比较总 Token、实际供应商账单、Worker CPU/内存与用户等待时间；只计算输出 Token 不算完成成本优化。 |
| NFR-016 | 同机连接并发 | 单 Windows 登录会话下维持 ≥64 个独立 Agent 客户端连接、16 个并行准备任务；与 1,000 逻辑身份/排队任务目标分开测量。 |
| NFR-017 | 并发一致性 | 在 500 个确定性同文件/跨文件并发场景中零静默覆盖；所有最终 Git/ref 或 Buffer 版本严格单调。 |
| NFR-018 | 公平性 | 持续高负载下配置为普通优先级的 95% 可执行作业不能因单 Agent 无限任务而永久饥饿；按容量基准评估。 |
| NFR-019 | 本地 IPC 安全 | 默认仅本登录 SID/受信进程可访问，拒绝远程客户；禁止跨 Windows 用户越权及意外提权。 |
| NFR-020 | 本地恢复 | 重启服务并重放 100 次注入故障，已提交作业零重复效果、未提交作业恢复/拒绝状态明确。 |
| NFR-021 | 资产覆盖 | 目录初始具有 23 个 GoF 模式条目、≥16 个工程模式及 ≥30 个经典算法知识卡；可执行模板逐条按语言认证，未认证仅展示知识。 |
| NFR-022 | 资产正确性 | 每个可执行算法有边界/随机/复杂度/属性测试证据；错误复杂度声明或测试不达标时拒绝发布。 |
| NFR-023 | IME 响应 | 本地已缓存候选 p95 ≤50ms，远程/模型候选不得阻塞键盘主线程；性能目标为基准而非当前成绩。 |
| NFR-024 | 输入隐私 | 密码域、保护输入、提权/安全桌面等不可访问上下文，默认不开启跨应用源码抓取；得到显式同意才发往模型。 |
| NFR-025 | 机器调用稳定性 | 模板与算法工具采用显式 JSON Schema 和版本标识，升级不静默破坏旧 Agent；旧协议回归 100% 通过。 |
| NFR-026 | 离线可用 | 本地 Hub 已安装目录和缓存命中条件下，断网可完成搜索、Preview 和获授权的本地编辑。 |
| NFR-027 | 并发可观测性 | 所有会话/作业带 agent_id、session_id、workspace_id、trace_id；脱敏后可定位锁等待与冲突原因。 |
| NFR-028 | 跨输入语言 | Rust/Python/TS 优先形成可执行模板；其他语言明确 capability matrix，汇编/机器语言默认禁止未经验证的字节级写入。 |

## 7. 安全、法规和许可

- **数据最小化**：仅传输与计划相关的符号和显式授权的上下文；密钥、`.env`、凭证目录默认拒绝采集。
- **受控执行**：模板本身视为不可信供应链资产。字符串替换不能防命令注入，需受限渲染器、路径白名单和隔离运行。
- **能力隔离**：Token 绑定租户/仓库/分支/动作/有效期；每次 Worker 侧重新验证。
- **审计与隐私**：默认不保留完整 prompt 或源码于公共日志，仅保留摘要、必要哈希与授权时可追溯引用；组织可设定保留期限。
- **许可**：本仓已声明 AGPL-3.0-only；服务器部署、再分发和模板输出的法律义务按实际依赖/模板许可独立评审，不提供“天然免费可商用”的保证。

## 8. 数据要件

- 必须保存 `Tenant`、`Workspace`、`Repository`、`TemplateVersion`、`SymbolIndex`、`Plan`、`Job`、`Lease`、`CommitRecord`、`AuditEvent`、`CostRecord`、`LocalAgentSession`、`AgentGrant`、`CatalogAsset`、`CatalogAssetVersion`、`CatalogImplementation`、`ImeProfile`。
- 仓库内容采用 Git 对象或不可变 snapshot URI；数据库中存对象摘要及访问控制，不保存重复大文本副本。
- 模板记录必须包含 `template_digest`、`version`、`engine_version`、`schema_digest`、`provenance`、`license`、`approval_status`。
- 作业表通过唯一索引锁定 `(tenant_id, idempotency_key)`，禁止跨租户误复用。

## 9. 验收流程及完成标准

1. 需求验收：P0 需求全部有实现引用、测试证据和签署结论。
2. 正确性验收：对中文/日文字符、并发同文件、共享 import 冲突、过期 base、恶意路径、重复投递、Worker 崩溃运行自动化回归。
3. 功能验收：分别在 CLI、Web、Tauri 和 Agent HTTP/stdio 客户端完成人类确认与纯 Agent 两种流程。
4. 基准验收：任务集至少 30 项、涉及模板适用和不适用两类；每任务至少 5 次，保留模型及价格版本，对比总成功成本、输出 Token、端到端耗时、编译/测试成功率。
5. 发布验收：完整 rollback runbook、告警路由、最小权限部署、安全扫描和版本兼容声明。

## 10. 需求风险与待决事项

| ID | 风险 / 待决 | 当前决定 |
|---|---|---|
| D-01 | Vercel 无法保证进程内共享文件系统 | Vercel 只承担 Router；正式修改在持久化 / 隔离 Worker 处理 |
| D-02 | 用户目标“无限 Agent” | 定义为无固定业务模型上限，但实施租户限流和可配置容量；不承诺无限资源 |
| D-03 | 同文件不同符号仍可能变更共享导入 | 提交时检测真实写入集合、重解析并检查类型依赖；必要时冲突 |
| D-04 | 模型是否必然节省成本 | 不保证；必须在真实模型、任务、缓存命中率和 Worker 价格下测量 |
| D-05 | 既有 `ide-kernel` 是否可直接支撑多文件 Git 事务 | 待开发适配层，禁止复制一个新的编辑缓冲作为真相来源 |
| D-06 | AST 引擎选型 | Rust 首选 `syn`/`quote` + 格式化，增量符号索引可用 Tree-sitter；TypeScript 后续适配 |

## 11. 版本范围与里程碑

| 里程碑 | 范围 | 出口标准 |
|---|---|---|
| M0 — 接口/门禁 | 规范、OpenAPI、Template Registry、策略与测试基座 | Schema 校验、鉴权、风险门禁通过 |
| M1 — 本地共享内核 MVP | Rust 模板、Local Hub、Agent 注册、多客户端只读查询、Plan/Preview/Apply | 16 个准备任务与基础冲突/身份隔离通过 |
| M2 — 同机并发与知识目录 | 公平调度、任务持久化、符号重基、GoF/工程模式/算法目录、MCP API | 64 连接、16 并发准备、无静默覆盖、目录完整性通过 |
| M3 — IME 与跨终端 | Windows TSF 输入法适配、F1 候选、Web/Tauri/TUI 联动、Vercel Router | 输入隐私、宿主兼容、跨终端 parity 及成本基准完成 |
| M4 — 多语言增强 | 其它语言模板认证、模式/算法补齐、审批/监控 | 语言能力矩阵与资产证据完成；稳定发行通过 |

## 12. 要件变更管理

新增/修改任何需求应同步更新 `(RD ID → BD 组件/接口 → DD 实现/算法 → TC 验收)`；破坏 API 须升级协议主版本。所有验收指标的阈值均为**设计目标**而不是当前性能声明。


## 13. 同一台电脑多个 Agent 并发使用：业务要件（RD-LOCAL）

### 13.1 部署与实例边界

系统应支持同一位开发者在一台 Windows PC 上，同时使用 Claude Code、Codex、Hermes、独立脚本、IDE1.0 Web/Tauri/TUI 等多个客户端。**默认采用每 Windows 登录用户一个常驻 Local Agent Hub**，由该 Hub 统一承载 Agent 注册、只读缓存、索引、租约、任务队列、提交协调，而不是一 Agent 一编辑器进程，也不是让多个 Agent 共用一个可写 Buffer。各进程仍拥有自己的 session / context / buffer view。

不同 Windows 登录用户不得默认共享进程权限或源码工作空间；确需跨用户或远程服务时使用明确的机器级服务部署并单独授权。允许多个 workspace 绑定到同一个 Hub，每个工作空间具有独立 policy、根目录与并发配额。

### 13.2 必须解决的冲突类型

| 类型 | 场景 | 验收行为 |
|---|---|---|
| Read/Read | 多个 Agent 查询模板和源码符号 | 完全并行，均获得指定快照的一致结果 |
| Read/Write | A 预览时 B 已提交 | A 的 Preview 保留快照来源；Apply 必须重新验证 |
| Write/Write — 同一符号 | A 与 B 同时替换一个函数 | 其中一个提交，其余返回冲突或形成新的受检计划 |
| Write/Write — 不同符号 | 同一文件改不同函数 | 可并行准备，提交串行化后重新解析、计算完整写集 |
| Shared Dependency | 不同函数引发公共导入、trait、类型变化 | 守住共享依赖边界；冲突时禁止静默合并 |
| Dirty Buffer | IDE 人类未保存内容，Agent 从磁盘计划 | 不得覆盖未保存修改；选择明确的基线或先进行双向协调 |
| Multi-file | Agent 同时更新 API+测试+配置 | 候选 Git tree 全量验证，通过一次 ref CAS 发布 |
| Offline vs Cloud | 本地 Hub 与云 Worker 同改仓库 | 采用 Git 对象、branch 版本和事务审计，失败后显式重基 |

### 13.3 角色与用例

- UC-10：用户在同一设备启动 20 个 Agent；所有 Agent 自动发现本地 Hub，分别注册 `agent_id` 与 `session_id`，获得独立请求额度。
- UC-11：一个 Agent 卡在长模型调用，其他 Agent 的模板搜索与已有任务查询仍可响应；不得持有工作区写锁等待模型输出。
- UC-12：Agent 被终止后，过期 lease 回收；其他 Agent 不承接其凭证；可恢复的任务重新排队。
- UC-13：机器离线时，模板搜索、知识卡、离线预览/编辑和本地审计正常运行；只有调用外部模型时按授权报错。
- UC-14：人类同时编辑同一文档时，Agent 只能对已确认的 Buffer snapshot 或 Git snapshot 申请修改；无法校验基线时要求重新预览。

## 14. 统一设计模式与经典算法目录：业务要件（RD-CATALOG）

### 14.1 GoF 模式全集（必须有知识卡，语言可执行模板按认证状态发布）

- 创建型（5）：Abstract Factory、Builder、Factory Method、Prototype、Singleton。
- 结构型（7）：Adapter、Bridge、Composite、Decorator、Facade、Flyweight、Proxy。
- 行为型（11）：Chain of Responsibility、Command、Interpreter、Iterator、Mediator、Memento、Observer、State、Strategy、Template Method、Visitor。

### 14.2 现代工程模式（首批不少于 16 项）

Dependency Injection、Repository、Unit of Work、CQRS、Event Sourcing、Saga、Transactional Outbox、Circuit Breaker、Bulkhead、Retry with Exponential Backoff and Jitter、Rate Limiter、Actor、Pipeline、Hexagonal Architecture、Clean Architecture、Publish–Subscribe、Idempotent Consumer、Cache-Aside、Object Pool、Read–Copy–Update。首批目录可收录 20 项，针对每项标明**适用条件、反例、并发风险、替代方案和系统边界**；现代模式并不全部属于 GoF 23 模式。

### 14.3 经典算法目录（首批 50 项）

| 分类 | 首批标识/算法 |
|---|---|
| 排序与选择 | merge_sort、heap_sort、quick_sort、quickselect、counting_sort、radix_sort、top_k_heap |
| 查找 | binary_search、lower_bound、upper_bound、two_pointers、sliding_window |
| 图 | bfs、dfs、topological_sort、dijkstra、bellman_ford、floyd_warshall、kruskal、prim、union_find |
| 字符串 | kmp、z_algorithm、trie、aho_corasick、rabin_karp、edit_distance |
| 动态规划 | knapsack_01、longest_increasing_subsequence、longest_common_subsequence、coin_change、interval_dp、bitmask_dp |
| 数据结构 | binary_heap、segment_tree、fenwick_tree、lru_cache、hash_table、monotonic_stack |
| 数学/数值 | euclidean_gcd、sieve_of_eratosthenes、fast_power、modular_exponentiation、extended_euclidean、reservoir_sampling |
| 并发/可靠性 | bounded_queue、producer_consumer、work_stealing、exponential_backoff_jitter、token_bucket |

同义算法名称统一为检索别名：`disjoint_set_union` 指向 `algorithm.graph.union_find`，`binary_exponentiation` 指向 `algorithm.math.fast_power`；不得通过别名虚增实际资产数量。绝不把教学算法实现等同于生产最优：**优先标准库/成熟库，密码学必须使用经过审查的密码库，不提供自写密码学实现**。算法知识卡应解释复杂度、稳定性、内存、溢出、极端输入、并发安全及限制。

### 14.4 资产状态与调用语义

知识卡有 `DOCUMENTED/REVIEWED`，语言实现额外有 `IMPLEMENTED/TESTED/APPROVED/DEPRECATED/REVOKED`；某算法在 Rust 已认证不代表 TypeScript 或 C 也已认证。Agent `catalog.search` 可发现知识卡，只有存在目标语言获批准实现且匹配约束，才能调用 `catalog.instantiate`。反例、许可和风险必须随机器结果返回，防止 Agent 为了减少 Token 无条件套用不适合的算法。

## 15. 程序员输入法与 Agent 调用的一体化边界（RD-IME）

- **单一知识库，两套适配**：Windows 程序员输入法（人类）和 `tase.catalog.*`/MCP（Agent）使用相同资产 ID、版本、语言矩阵、评分与测试证据；**Agent 不通过 TSF 注入键盘来调用模板**。
- 输入法在 Windows 切换列表中作为独立输入法；默认专注代码输入，不替代中文/日文一般文字输入法。
- 首批展示语言 Rust/Python/TypeScript/PowerShell/C/汇编/机器语言；其中 Rust/Python/TS 可优先做自动展开；PowerShell/C 逐步增加成熟模板；汇编需要指定 ISA/汇编器/ABI；机器语言仅以明确指令集和签名校验的低级片段进行高风险受控操作，默认提供知识/解释/只读候选，不直接写二进制。
- 手动选择语言、上下文自动识别、F1（可重映射）打开完整修改方案与当前位置插入候选；模糊候选输入时不阻塞键盘，也不抢占宿主软件的 F1。
- 默认不采集密码控件、安全桌面、其他程序的全文件内容或任意前台窗口文字；输入法只使用可获授权的编辑上下文，向外部 LLM 传输需要明确许可。
- TSF COM DLL 进程内部分保持极简，后台算法/模型、模板注册和调度在同一用户的 Hub 进程。Windows 安装/卸载、隐私和冲突处理需独立系统验收。

## 16. 正式版本验收边界

新增用例 TC-021～TC-049 与既有 TC-001～TC-020 构成完整计划测试。发布时要求每一项 P0/NFR 均能从 `.xlsx` 追踪到 BD、DD、TC，测试状态必须从“待实现”经真实运行与证据更新，不能将文档设计完成当作功能已经通过。更多验收设计见详细设计 `DD-LOCAL`、`DD-CATALOG` 和 `DD-IME`。
