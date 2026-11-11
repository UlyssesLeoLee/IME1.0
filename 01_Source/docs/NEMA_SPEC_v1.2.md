# .NEMA v1.2 — 统一规范（外部参考，抄自官方发布页）

> **来源**：<https://nema-thread.vercel.app/website/download.html>（.NEMA 的解释器与资源页）
> 规范原文：<https://nema-thread.vercel.app/docs/SPEC_v1.2_zh.md>
> IME1.0 接入设计：<https://nema-thread.vercel.app/docs/IME1.0_INTEGRATION_v1.2.md>
> IDE1.0 接入设计：<https://nema-thread.vercel.app/docs/IDE1.0_INTEGRATION_v1.2.md>
> 本文件是**外部参考的副本**，**不是本仓库的作品**：它是「多 Agent 演示模式」与 `crates/pime-core/src/nema.rs` 依据的规则原文。
> 官方声明：**候选协议 / 参考解释器已执行测试；未成为标准化组织发布的行业规范**；真实 Rust Codec、Windows TSF 与 IDE/IME 产品集成是**后续开发项**。

---

# .NEMA v1.2 — 统一规范（现行版）

- 文档状态：候选协议 / 参考解释器已执行测试；未成为标准化组织发布的行业规范。
- 本版兼容性：v1.2 Graph 节点改为**行首恰好一个 ASCII 空格**。旧版 `A \=LoginAPI` 必须经迁移处理，不可当成 v1.2 Node。
- 此文档定义 `.nema` **编辑层**；`.nemat` 是可逆的压缩传输层。源码（Rust、TS 等）保持原格式，`.NEMA` 只存储协作和图拓扑元数据。

## 1. 核心语义

**横向不规则、纵向按需延展**。ASCII 空格（U+0020）定位 X 列；换行 LF 定义 Y 意群。语义上没有必要时不强制拆句。中日文可以直接横向连写；英文推荐 `BuildLoginFlow`、`HTTPItIs`，多段无法区分的全大写缩写应换行。`HTTPItIs` 展示为 HTTP it is，但不保证逆向恢复原始空格与大小写。

一个领域头总是 `Tab + 领域ID + 空格 + 正整数修订号`。Graph 头是 `Tab + > + 领域ID + 空格 + 修订号`。Tab 不再充当正文列分隔。领域之间**无空行**。整个文档以连续两个完全空白的物理行结束（最后正文行后 3 个 LF）。不允许 CRLF 或额外的空行。

## 2. 三模式

| 模式 | 判定 | X0、X1、X2 |
|---|---|---|
| 5W1H（默认） | 领域头后第一条正文行顶格 | What、Why、How；X3=Who、X4=When、X5=Where |
| Table | 领域头后第一条正文行以单个 Tab 开始 | 该行是一组自定义列名；记录首列非空则开新记录 |
| Graph | 领域头带 `>` | Graph 内部的 Node、Edge 使用独立的两种 X0 声明方式 |

## 3. 5W1H / 纯文本

```text
⇥Ttext1 1
What
HTTPItIs
⇥Tzh1 1
审核认证流程 防止泄密 验证令牌


```

其中 `⇥` **仅在文档说明中代替一个真实 Tab**；可执行文件必须使用 ASCII Tab。字段顺序永远是 **What → Why → How → Who → When → Where**。空格分列；如果只有一个字段就是 What 文本。`What` 与 `HTTPItIs` 是同一 What 的两个纵向意群，可渲染为 `What HTTP it is`。

## 4. Table

```text
⇥Tab1 1
⇥Agent Status Note
Claude Ready 校验完成
  等待再次复核
Codex Blocked HTTP401


```

`⇥` 是真实 Tab 的可视替身；`␠␠` 在文中可用于强调两个空格。`  等待再次复核` 从 X2 纵向续写到上一条 Claude 的 Note。普通数据行首列非空，才是新行。数据的自然语言空格不能直接与分列空格混淆；可使用 PascalCase、合适的纵向意群或 `\ ` 表示精确单元格内字面空格。

## 5. Graph / Node–Edge

```text
⇥>Gauth1 1
␠A LoginAPI Authentication
␠B TokenStore SessionStorage
A>X>B Reads VerifySession
␠␠CacheExpiredRetryOnce
B<Y<A Responds SessionReady
A=U=B Shares SecurityPolicy
A>P>A Retry Maximum3Attempts


```

上述 `⇥` / `␠` 在教程中是显示用标记；**实际文件中只写一个 Tab 与 ASCII 空格本身**。

- `␠A`：Graph 中**行首恰好一个空格**，创建 Node A；X1=`LoginAPI`（标识符原样保留，无需 `\=`），X2=`Authentication` 是可选职责说明。
- `A>X>B`：没有前导空格，创建 Edge X，从 A → B；X1=`Reads`，X2=`VerifySession`。
- `B<Y<A`：创建反向 Edge Y，实际 A → B。解析器永远以 X0 的箭头符决定方向。
- `A=U=B`：无向关联 U；双向独立行为请写两条方向相反的 Edge。
- `A>P>A`：自循环；允许平行边、反馈环、多 Agent 依赖与普通无环 DAG（后者需要另做有环校验）。
- `␠␠CacheExpiredRetryOnce`：**恰好两个前置空格**，继续当前 Node/Edge 的 X2；更多前置空格延伸到更右侧的扩展属性。
- Node 和 Edge ID 在单个 Graph 内唯一，边两端必须引用已声明的节点。Graph 行首零空格只能写 Edge，一空格只能写 Node，两及以上是纵向续写。X1 在 v1.2 不允许纵向续写，以免与 Node 声明歧义。

**X2 只影响描述和 AI 语义，不会创建节点、改变 Edge 箭头。** 悬停 Node/Edge 时实时显示其 X2；点击固定完整详情。键盘焦点以及触屏单击同等可用。

## 6. 源文本的空格与转义

- 原文件中的 U+0020 空格要真实保留，禁止通过 CSS 字体/布局伪造多个空格；示例 `<pre>` 必须使用等宽字体、`white-space:pre`、`letter-spacing:0`，禁用连字。
- UI 的 `·` / `⇥` 可视化开关仅改变**显示副本**。复制/保存必须使用原始原文。
- 原始精确空格需写 `\ `，反斜杠为 `\\`，原始 Tab 作为正文值用 `\t`。5W1H/Table 中的 `\=literal` 可提示按字面保留；Graph X1 默认按字面保留。
- 无法从 `LoginAPI` 推断是否应变成 `Login API`；Graph 名称默认不拆。普通英文 PascalCase 只用于人类语义显示。
- 若需要保留人类原文的每一个空格/大小写/换行，使用已经准确记录的 `.nema` 原文，不要从语义视图倒推原文。

## 7. 领域头保留与边界

`^\t>?ID REV$`（详见 `codec/native.py` 的严格正则）永远优先于正文，防止无空行时丢失边界。这是**保留的结构形状**，普通文本如果恰巧长成领域头，需要由编辑器提示/修改。不能靠 LLM 猜测。一个 Graph 领域与另一个 Table 领域无需空行分隔。

## 8. 语义不等于字节无损

人类渲染 `BuildLoginFlow` → “Build login flow” 是展示策略；AI 生成的原生格式不保证原自然语言的 byte-exact roundtrip。而 `.nemat` 编码—解码对**已存在的 `.nema` 原始 UTF-8 字节**提供完全无损保护。在 API 调用前应通过同一解释器验码，不能只依赖模型学习。

## 9. 最低兼容规则

编码版本 `v1.2`、UTF-8、LF、三模式、WHAT-first、Graph leading-space Node、Edge 方向与悬停 X2、无损 `.nemat` 必须明确标记。旧版 Graph 原文不能混进当前训练集。在转换失败时回退原文，显示语法错误，不自动“修正”代码文件。

## 10. Graph X3+ 扩展属性的语义输出（R7 勘误）

Graph 的 X3、X4… 不是仅供解释器内部存储：`codec.native.parse()` 保留列原子；`codec.native.view()` **必须**将有值的扩展列输出为可选 `extensions` 映射（例如 `{"X3":"APIKey"}`）。无扩展列时不生成该键，以保证现有 JSON 语义输出兼容。X3+ 使用精确字面量拼接，不自动拆分 `HTTP401`、`SHA256` 或 `APIKey`。纵向续写 X3 是三个前导空格，X4 是四个，依此类推；X2 的两个前导空格规则不变。

```text
⇥>G1 1
␠A LoginAPI Authentication APIKey
␠B TokenStore SessionStorage
A>X>B Reads VerifySession HTTP401
␠␠␠RetryOnTimeout


```

`⇥`/`␠` 在文档中是**展示用标识**；真实 `.nema` 使用 Tab 与 ASCII 空格。Graph A 节点 X3 保留 `APIKey`，Edge X 的 X3 呈现 `HTTP401 RetryOnTimeout`。其 X2 始终只用于说明和悬停，不改变关系方向。

注意：Graph 旧式 `A \=LoginAPI` 只可在迁移指南和历史对比中出现，当前原生 Graph 节点必须以 **恰好一个 ASCII 空格**起始。

---

## 本仓库怎么用它（两处）

| 落点 | 干什么 |
|---|---|
| `crates/pime-core/src/nema.rs` | 领域头的识别与领域范围的划分（`Tab + 领域ID + 空格 + 修订号`；一个领域管到下一个领域头之前） |
| `crates/pime-tsf/src/demo.rs` | 多 Agent 演示：10 个 Agent 各自在自己的领域内写 20 个字符；文本里没有领域头时补上本页的下载链接 |

**本仓库没有实现 .NEMA 的解释器与无损 Codec**（那是官方发布的 Python 参考实现，下载页在上面）；本仓库实现的是**演示模式**与**领域结构的识别**，语义以官方解释器为准。
