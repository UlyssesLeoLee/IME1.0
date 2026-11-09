# TASE 在本仓库的链路：谁跟谁说话，哪一段还是断的

> 这份文档画一条线：**AI 客户端的一句话，怎么变成用户文档里的一段文字**。
> 每一段都标了它在哪个 crate、走过什么通道、**现在通不通**。
> 没有通的段落，写清楚「差什么」——不拿「理论上可以」充数。

## 一、整条链路

```
  AI 客户端（Claude Code / 其它 MCP 客户端）
        │  MCP over stdio
        ▼
  pime-agent（产品的 AI 前端：MCP 服务端 / CLI / 本地管道 / HTTP）
        │
        ├── catalog.search / recommend / instantiate / describe
        │      └── 读磁盘上的目录（93 张卡 + 50 条实现登记），不走输入法
        │
        ├── tase_preview ──┐   ← 预览：算 diff，什么都不写
        │                  │
        └── tase_apply ────┤   ← 应用：一次读 + 一次写
                           │
                           ▼
              ┌──────────────────────────────┐
              │  tase-executor（执行器）      │
              │  · 受约束 diff               │
              │  · 逆操作（撤销描述）         │
              │  · 宿主门 host.rs             │
              │    HostTransport ← 接缝      │
              └──────────────────────────────┘
                           ▲
                           │  实现：pime-agent/src/host_transport.rs
                           │  （用桥接客户端 WM_COPYDATA 找焦点窗口里的桥接窗）
                           ▼
              ┌──────────────────────────────┐
              │  pime-tsf（TSF DLL，输入法）    │
              │  · Op::Read   读焦点窗口的文本 │
              │  · Op::Insert 一次写入          │
              │  · 安全闸门：密码框不写         │
              └──────────────────────────────┘
                           │
                           ▼
                 用户正在编辑的文档（记事本 / VS Code / …）
```

## 二、还有一条「重活」的路：Hub 与 worker

```
  AI 客户端
      │  交一个作业（job.enqueue）
      ▼
  tase-local-hub（常驻 Hub：一个登录会话一个）
      │  · 鉴权、会话、工作区的写锁
      │  · 调度器：五类分层队列 + 每 Agent 加权公平 + 租约与 fencing
      │    （真实现：tase-agent-scheduler；轮转是降级实现）
      │  · 持久化：SQLite WAL（tase-local-store）
      │
      │  job.take：worker 来要作业（带这一轮的 fencing token）
      ▼
  tase-worker（作业执行）
      │  · 消费者、租约、fencing、恢复（幂等键）、验收门禁（ACI）
      │  · 算的是 **diff**：不写用户的窗口
      │  · 写是客户端那边的事（tase_apply）
      ▼
  job.complete（带 token；token 不对 ⇒ 拒绝）
      → 客户端用 job.status 取结果
```

## 三、每一段的状态

| 段 | 状态 | 证据 | 还差什么 |
|---|---|---|---|
| 客户端 → `pime-agent`（MCP） | **通** | `pime-agent` 74 项测试；`catalog.*` 四个工具 + `tase_preview` / `tase_apply` 两个工具 | —— |
| `catalog.*`（读目录） | **通** | 93 张卡 + 50 条实现登记；`validate-tase-catalog.js` 0 问题 | 实现的状态停在 `TESTED`（`APPROVED` 要人签字 → D-61） |
| `tase_preview`（预览 diff） | **通** | 单元测试：编辑套用、UTF-16 偏移、非法偏移被拒 | —— |
| `tase_apply`（真写） | **通（代码层）** | 单元测试：一条读 + **一条**写、`replace=true`、拒绝与失败分开 | **没在真机跑过**（新 DLL 要注销/重启才换上） |
| 执行器 → 宿主 | **通** | `tase-executor` 68 项；`BridgeHost` + `HostTransport` | 适配器改的是**选区**，不是磁盘文件（设计如此，已写进文档） |
| 宿主 → 文档 | **通（十七轮已证）** | 真机 `WM_COPYDATA` 往返（`pime-agent` 侧） | 本轮**没重测**（真机待验收） |
| 客户端 → Hub（交作业） | **通** | `job.enqueue` / `job.status` 已在 Hub 里 | —— |
| Hub → worker（取作业） | **通** | `job.take`：一次只给一个作业，带这一轮的 fencing token；`job.complete`：token 不对就拒 | —— |
| worker → Hub（回报结果） | **通** | `tase-worker` 的 `hub_link.rs`：管道客户端（`MemoryPipe` 测、`WindowsPipe` 真），一次完整往返有测试 | —— |
| worker → 用户窗口 | **不通（有意）** | worker 没有宿主 | worker 只算 diff；写由客户端做（设计如此） |

## 四、还没通的那一段（说清楚）

* **Hub 的 `next_job` 以前只在测试里被调用**：作业进了队列就躺在那儿，生产路径上没人取。
  ⇒ 本轮补齐了：`job.take`（worker 来要作业，**一次只给一个**，带这一轮的 fencing token）、`job.complete`
  （**token 不对就拒**）、`job.status` 能拿到做完的结果。
* **fencing 在 Hub 这一层校验**：store 的 `finish_job` 不校验 lease epoch（**D-67**），所以 Hub 是最后一道防线。
* **worker 的二进制现在真的接上了**：它连的是 Hub 的管道，取作业、跑（**只算 diff，不写用户的窗口**）、回报；
  **Hub 不在时它打印一句人话就退出，不假装在工作**。
* **仍然要你裁定**：**D-63**（Hub 的文件锁没有 fencing token）、**D-64**（租约职责重叠：worker 与 scheduler 都写着「租约与 fencing」）。
  这两条不裁定，队列上的「谁有权写」还是含糊的。

## 五、这套东西**不做**什么（不吹）

* **不联网**：没有 API key，不调任何服务商；目录、计划、预览、应用全在本地。
* **不改协议**：0.3 的协议是冻结的；宿主门用的是**现有的两个 op**（`read` / `insert`），
  `pime-core/src/agent_proto.rs`、`schemas/core-protocol-v0.3.json` 与整个 `crates/pime-tsf/` **零改动**。
* **不假装写成功**：宿主拒绝（密码框/没焦点/输入法不在）⇒ `Rejected`；宿主没给确定答案 ⇒ `Failed`（结果未知）。
  worker 没有宿主时**直接拒单**，不假装它在工作。
* **没有真机验收**：这一轮的所有「通」都是**代码层 + 单元测试**；真实宿主里跑一遍要等新 DLL 换上（注销/重启）。
