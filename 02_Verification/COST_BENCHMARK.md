# 成本基准与延迟基准（第二十二轮，2026-11-09）

> 这一份是 M4「可视化与成本基准」在本仓库的落点：**两个数**——本地候选路径的**延迟**、一次模型调用的**成本**。
> 两者都用**产品自己的口径**测：延迟量的是 `pime-core` 的候选引擎真实调用；token 用核心自己数 token 的那条规则
> （`ceil(UTF-16 长度 / 3)`，见 `pime-core/src/budget.rs` 的 `estimate_tokens`），价格用 `tase-metrics` 的价格表算。
>
> **原始证据**：`02_Verification/cost-benchmark.json`（每次跑都重写，所以**不在**完整性清单里——它和日志一样是每次变的）。
> **跑法**：`node 01_Source/scripts/bench-cost.js`（门禁里也跑这一步，见 `run-all.js` 的 `bench` 组）。
>
> **它和仓库里已有的 UT-068 是一对，不重复**：
> UT-068（`01_Source/tests/rust/benchmark.rs`）是**单元测试**里的诊断测量，门槛更严（**p95 ≤ 20 ms** 的内部门限，
> 1000 个前缀事件）；本文件是**证据**：把延迟与成本写进 `02_Verification/`，并按**设计书**的口径（DD-IME §20.2 的
> `p95 <= 50 ms`）判定。两个门槛都在，一个管回归，一个管交付证据。

## 一、延迟：本地候选路径

**预算**（设计书 DD-IME §20.2）：本地缓存的候选必须够快，`p95 <= 50 ms`。

| | 2026-11-09 实测（本机） |
|---|---:|
| 样本 | 1200 次（6 个日常前缀 × 200 轮） |
| p50 | **0.3 µs** |
| p95 | **28.5 µs** |
| max | 60.9 µs |
| 预算 | 50 000 µs（50 ms） |
| 余量 | p95 比预算低 **约 1750 倍** |

**测的是什么**：三种语言（zh-CN / ja-JP / en-US）× 六个日常前缀（`ni`、`shi`、`konnichiiha`、`fn`、`let `、`impl `）
的真实 `engine().predict(...)` 调用，**在进程内**（不含 IPC、不含 TSF、不含界面绘制）。这是**候选引擎本身**的口径，
不是「按键到上屏」的端到端口径——后者要等真机验收（见下）。

**怎么自己复跑**：

```powershell
$env:PATH="C:\Users\seiko\.cargo\bin;C:\msys64\mingw64\bin;"+$env:PATH
$env:RUSTUP_TOOLCHAIN="stable-x86_64-pc-windows-gnu"
$env:CARGO_TARGET_DIR="C:\cargo-target-ime\tase-metrics"
cd 01_Source; node scripts\bench-cost.js
```

**门禁怎么用它**：`run-all.js` 的 `bench` 组跑同一个脚本，**p95 超预算就让门禁红**（这是唯一一处把**时间**变成判据的地方，
余量 1750 倍，所以它不会因为机器忙而抖动；真抖了要看的是机器，不是产品）。
**单元测试里还有一道更严的**：UT-068（`tests/rust/benchmark.rs`）要求 **p95 ≤ 20 ms**（内部门限，1000 个前缀事件）——
它管回归，这份基准管交付证据。

## 二、成本：一次模型调用

**口径**：一次典型请求 = 产品发给模型的东西（指令 + 光标周围的代码 + 用户的问题），按核心自己的规则数 token；
输出按「产品只要一个短答案」估 200 token；价格用 `tase-metrics` 的 `PriceTable` 算。

| | 值 |
|---|---:|
| 输入 token | **96** |
| 输出 token（估） | 200 |
| 价格（**样例表** sample/sample-model-small） | **0.0001344 USD** |
| 价格（**样例表** sample/sample-model-large） | **0.003288 USD** |

### ⚠️ 这些价格是**样例**，不是任何服务商的报价

`01_Source/assets/pricing/sample-price-table.json` 里是**样例数字**，provider 写的就是 `sample`。
它只是让基准有个可运行的形状。**要你自己的数字**：把那个文件换成你的价目表（形状见文件里的 `note`），
或者直接 `node 01_Source/scripts/bench-cost.js <你的价目表.json>`，报告里的 `cost.models` 就是按你的表算出来的。

**没有做的事**（不虚报）：
- **没有**测真实服务商的账单——本仓库**不联网**，也没有任何 API key；
- **没有**测「一次真实编辑」会发多少 token——那取决于用户在编辑器里选了多少代码（**输入**部分），
  基准用的是一段固定的典型代码；
- **没有**测缓存命中的价格优势（表里有 `cached_input_per_million`，但基准没走缓存路径）。

## 三、还没测的（要真机）

| 口径 | 状态 |
|---|---|
| 按键 → 候选 → 上屏的**端到端**延迟（含 TSF、含界面） | **未测**：新 DLL 还没换上（要注销/重启） |
| 多 Agent 同时写入时的**争用**延迟 | **未测**：调度器与 worker 还没接线（D-63 / D-64 待裁定） |
| 真实服务商的 token 账单 | **未测**：本机不联网、无 key |
| 长时间（30 分钟）的**延迟漂移** | **未测**：设计书的离线长稳用例仍未执行 |

## 四、这一轮为做这件事改了什么（很小，都是加法）

| 改动 | 说明 |
|---|---|
| `pime-core/src/budget.rs` | **加**一行 `pub use crate::ai::estimate_tokens;`——基准要用**产品自己**数 token 的那条规则，不另抄一份 |
| `crates/tase-metrics/examples/cost_bench.rs`（新） | 基准本体：延迟 + token + 成本，打印一份 JSON |
| `assets/pricing/sample-price-table.json`（新） | **样例**价目表（provider 写 `sample`），附说明 |
| `scripts/bench-cost.js`（新） | 驱动脚本：跑基准、写证据、查预算 |
| `tests/run-all.js` | 门禁多一步：`bench` 组（p95 超预算即红） |
| `scripts/update-manifest.js` | 清单里登记新文件；**证据文件** `cost-benchmark.json` 与日志一样**排除**（每次重写） |
