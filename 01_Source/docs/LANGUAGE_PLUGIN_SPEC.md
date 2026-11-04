# 插件语言通用规范（Language Plugin Specification）

> 版本：1.0 · 状态：随 0.3.0 发布 · 适用产品：IME1.0（编程输入法）
>
> 本文件是「语言包以插件形式导入」的**唯一权威规范**。任何插件包（发行版随附的、用户自己写的）都必须满足本规范；
> 两个引擎（VS Code 桥接的 JavaScript 引擎、Windows 输入法的 Rust 核心）按同一份规范实现，并由共享夹具互相校验。

---

## 1. 目的与范围

输入法需要能理解用户正在写的**编程语言**。产品自带 7 种语言（Rust / Python / TypeScript / PowerShell / C / 汇编 / 机器码），
但真实工程里还有别的语言：SQL 方言（Cypher、PL/SQL、T-SQL）、Go、Java、Kotlin……

本规范解决三件事：

1. **新增一整门语言**（不只是给已有语言加触发词）——用「插件包」描述；
2. **用户可以自己导入**——把一个 `.json` 放进插件目录即可，不用等发行版；
3. **开关可控**——内置 7 种恒开；插件语言默认开一部分（T-SQL 关、PL/SQL 开），用户随时可以打开。

### 1.1 不在本规范范围内

- **AI 生成的候选**：与语言包无关，走既有的 AI 通道。
- **符号索引（F1 注释）的语言规则**：当前只对内置 7 种语言有声明式规则；插件语言没有规则时该列表为空（见 §8.3）。
- **执行**：插件包是**纯数据**，永远不会被执行。包里没有表达式求值、没有钩子、没有模板求值。

---

## 2. 术语

| 术语 | 含义 |
|---|---|
| **profile（语言档案）** | 一个候选生成规则集的唯一标识，例如 `rust`、`cypher`。候选请求、锁、方言都以 profile id 为准。 |
| **内置 profile** | 产品自带的 7 个，**恒开、不可关闭**：`rust` `python` `typescript` `powershell` `c` `assembly` `machine-code` |
| **插件 profile** | 由插件包新增的 profile，可以是发行版随附的（`cypher` `plsql` `tsql`），也可以是用户导入的。**可开可关。** |
| **插件包（plugin pack）** | 描述一个插件 profile 的 JSON 文件。 |
| **用户规则包（user rule pack）** | 既有功能：只能给**已加载**的 profile 追加触发词/关键词，不能新增语言。目录是 `<globalStorage>/rules/`。 |
| **方言轴（dialect axis）** | 包内 `dialects` 的一个维度，例如 `version`、`edition`、`std`。每个轴有一个 `default`。 |
| **验证器种类（validator kind）** | `validator` 字段的取值，决定用哪一族结构检查器。 |

> **插件包 ≠ 用户规则包。** 两者文件形状不同、目录不同、语义不同：
> - 插件包 = **完整包**（和内置包同形状），可以**新增一整门语言**；
> - 用户规则包 = **增量包**（只有 `schema_version` / `profile_id` / `version` / `keywords` / `triggers`），只能**扩充**已有语言。
>
> 两者互不替代：想给 Rust 加触发词用规则包；想支持一门新语言用插件包。

---

## 3. 插件包格式

### 3.1 目录

| 位置 | 内容 | 谁来放 |
|---|---|---|
| `01_Source/language-plugins/*.json` | **发行版随附**的插件包（`cypher.json` / `plsql.json` / `tsql.json`） | 产品发行 |
| `<globalStorage>/language-plugins/*.json` | **用户导入**的插件包 | 用户（通过「导入语言插件」命令） |
| `01_Source/language-packs/*.json` | 内置 7 种语言的包 | 产品发行（**只有 7 个文件**） |

打包时 `language-plugins/` 必须与 `language-packs/` 一起进 `.vsix` 和安装包。

### 3.2 字段表

插件包与内置包**字段完全相同**：

| 字段 | 类型 | 必填 | 约束 |
|---|---|---|---|
| `schema_version` | number | 是 | 必须是 `1`。不认识的值直接拒绝，不做猜测迁移。 |
| `profile_id` | string | 是 | 插件：`^[a-z][a-z0-9-]{0,31}$`，且**不得**与内置 profile id 相同；内置：必须是内置 id 之一。 |
| `version` | string | 是 | 语义化版本 `^\d+\.\d+\.\d+$`。 |
| `display_name` | string | 是 | 人读的名称（例：`PL/SQL`）。 |
| `language_ids` | string[] | 是 | 编辑器的语言 id（例：`plsql`）。**不得**占用内置包已占用的 id。 |
| `accepted_extensions` | string[] | 是 | 带点的扩展名（例：`.pls`）。**不得**占用内置包已占用的扩展名。 |
| `validator` | string | 是 | 验证器**种类**名，见 §5。 |
| `dialects` | object | 是 | `{ 轴名: { values: string[], default: string } }`，`default` 必须在 `values` 里。轴值按**从旧到新**排列（版本轴靠这个顺序判断特性可用性）。 |
| `keywords` | string[] | 是 | 关键字表。`^[\w#.$-]{1,64}$`。重复项会被去重。 |
| `triggers` | object[] | 是 | 触发规则，见 §3.4。 |

### 3.3 示例（节选）

```json
{
  "schema_version": 1,
  "profile_id": "plsql",
  "version": "1.0.0",
  "display_name": "PL/SQL",
  "language_ids": ["plsql"],
  "accepted_extensions": [".pls", ".plsql", ".pkb", ".pks", ".sql"],
  "validator": "sql",
  "dialects": { "version": { "values": ["12c", "18c", "19c", "21c", "23ai"], "default": "21c" } },
  "keywords": ["begin", "end", "declare", "exception", "cursor", ...],
  "triggers": [
    {
      "id": "plsql.begin",
      "pattern": "begin",
      "kind": "syntax",
      "weight": 90,
      "contexts": ["statement"],
      "candidates": [
        { "label": "anonymous block", "text": "BEGIN\n    ${cursor}\nEND;" }
      ]
    }
  ]
}
```

### 3.4 触发规则

| 字段 | 含义 |
|---|---|
| `id` | 包内唯一，`^[\w.-]{1,80}$` |
| `pattern` | 用户输入的前缀，`^[A-Za-z0-9_#.-]{1,64}$` |
| `kind` | `syntax`（默认）或 `prefix` |
| `weight` | 0..100，参与排序 |
| `when` | 方言条件：`{ 轴名: [允许的值] }` |
| `contexts` | 上下文：`statement` / `expression` / `type` / `unknown` / `literal` |
| `in` / `not_in` | 所在块类型（如 `class` / `def` / `module`） |
| `candidates` | 1..8 条候选，每条 `{ label, explain, text, stmt_semicolon, assumptions, when }` |

候选文本里只允许 `${占位符}` 与 `${cursor}` 两种占位形式。`${` 后面不是合法占位符名 → 整包拒绝。
**包里不允许出现任何可执行内容。**

---

## 4. 校验规则（两个引擎必须一致）

1. 未知字段 → 拒绝（`RULEPACK_INVALID`）。
2. `schema_version != 1` → 拒绝，提示需要迁移。
3. `profile_id` 不合法或与已有 profile 冲突 → 拒绝。
4. 插件占用**内置**的 `language_id` 或扩展名 → 拒绝（防止劫持已有语言）。
5. 大小上限 256 KiB；触发词 ≤ 2000；候选 ≤ 8/条；候选文本 ≤ 4000 字符；标签 ≤ 200。
6. 关键词必须匹配 `^[\w#.$-]{1,64}$`。
7. 方言轴必须有合法的 `default`。
8. **任何一条不满足 → 整包拒绝并报出原因**；一个坏包不得影响其它包，也不得降级已有规则。

---

## 5. 验证器种类（validator kind）

`validator` 字段是**种类名**，由它派发到一族结构检查器。

| 种类 | 覆盖 | 说明 |
|---|---|---|
| `sql` | `cypher` / `plsql` / `tsql` | SQL 族检查器（§5.1） |
| `c-like` | `rust` / `c` / `typescript` | 内置包沿用既有取值，**行为不变** |
| `python-indent` | `python` | 同上 |
| `powershell` | `powershell` | 同上 |
| `assembly` | `assembly` | 同上 |
| `hex` | `machine-code` | 同上 |

- 内置包的既有取值**不是**种类名，因此内置包继续按 profile id 派发，**行为零变化**。
- 未知种类 → 结果为 `NO_VALIDATOR` / `unchecked`（**绝不**崩、**绝不**误判为 `valid`）。

### 5.1 SQL 族检查器（`sql`）

**遮蔽规则**（扫描左→右）：

- `--` → 行注释直到行尾；
- `/*` → 块注释直到下一个 `*/`（不嵌套）；
- `'` → 字符串，`''` 是转义引号（不结束字符串）；
- `"` → 双引号标识符，`""` 是转义引号；
- 其余是代码。遮蔽后保留所有换行、被遮蔽字符替换为空格。

**检查顺序**：

1. `SQL_UNTERMINATED_STRING`（行号 = 起始引号所在行）：引号直到文本结束仍未闭合。
2. 括号问题（复用既有括号检查器，码与顺序一致）：`BRACKET_UNEXPECTED` / `BRACKET_MISMATCH` / `BRACKET_UNCLOSED`。
3. 方言版本检查（仅当 `version` 轴的值在该 profile 的已知有序列表里）：

   | profile | 版本轴 | 检查 | 码 | 所需版本下标 |
   |---|---|---|---|---|
   | `cypher` | `4`,`5` | `EXISTS` 后跟 `{` | `CYPHER5_EXISTS_SUBQUERY` | ≥ 1 |
   | `cypher` | | `IN TRANSACTIONS` | `CYPHER5_IN_TRANSACTIONS` | ≥ 1 |
   | `plsql` | `12c`,`18c`,`19c`,`21c`,`23ai` | `LISTAGG` | `PLSQL19C_LISTAGG` | ≥ 2 |
   | `tsql` | `2012`,`2016`,`2017`,`2019`,`2022` | `CREATE OR ALTER` | `TSQL2016_CREATE_OR_ALTER` | ≥ 1 |
   | `tsql` | | `STRING_AGG` 后跟 `(` | `TSQL2017_STRING_AGG` | ≥ 2 |

   仅当请求版本的下标**低于**所需下标时触发。匹配大小写不敏感，在**遮蔽后**的文本上做，整词匹配，行号取匹配所在行。

4. 结果：无问题 → `ok=true` / `valid`；否则 `ok=false` / `invalid`。

> JavaScript 与 Rust 两份实现必须**逐字节一致**（同样的码、同样的行号、同样的顺序）。两边的检查结果由共享夹具 `tests/fixtures/validate-cases.json` 对拍，任何一边漂移都会让门禁变红。

---

## 6. 启停模型

### 6.1 集合

| 集合 | 内容 | 默认 |
|---|---|---|
| 内置 | 7 个内置 profile | 恒开 |
| 发行版随附的插件 | `cypher` `plsql` `tsql` | 见下 |
| 用户导入的插件 | 用户放进来的一切 | 默认**关**（用户导入后自行打开） |

**默认启用的插件集合 = `["cypher", "plsql"]`** —— 即 **T-SQL 默认关闭、PL/SQL 默认打开**（产品需求）。
`cypher` 是第一个默认插件，与 PL/SQL 一起默认打开。

### 6.2 开关落在哪一层

开关由**宿主产品**持有，不由包持有：

| 宿主 | 存储位置 | 生效方式 |
|---|---|---|
| VS Code 桥接 | 设置 `programmingIme.pluginLanguages`（字符串数组） | 重建注册表；并通过协议 op `setPluginLanguages` 同步给 Rust 核心 |
| Windows 输入法（TSF） | 进程内核心的启用集合 | 由文本服务在激活时按所选代码语言启用 |

**包本身不带「开/关」字段** —— 一个包可以同时被多个宿主加载，各宿主自行决定启用集合。

### 6.3 关闭的包必须「可见但无效」

关闭的插件包**仍然能被列出来**（用户要在设置里看到它、才能打开它），但它对运行时**零贡献**：
不进候选注册表、不占语言 id、不占扩展名、不产生触发词。

---

## 7. 扩展名归属与语言解析

一个扩展名**可以**被多个插件占用：`plsql` 与 `tsql` 都占用 `.sql`。

解析顺序（与既有行为一致）：

1. **内置硬编码表优先**（`STRONG_EXT` / `AMBIGUOUS_EXT`）——内置语言的行为完全不变。
2. 之后由**注册表**决定（注册表里只有**已启用**的插件）：
   - 恰好 1 个归属 → 强证据（置信度 85，原因 `extension`）；
   - 多个归属 → **歧义**：保持当前语言不变，标记 `uncertain`，`suggestion` 给第一个归属；由既有的「手动语言锁」界面让用户选；
   - 没有归属 → 既有的 `ambiguous-extension` / `kept` 行为。

实际效果（默认集合 `cypher` + `plsql`）：

| 文件 | 解析结果 |
|---|---|
| `x.cypher` / `x.cql` | `cypher`（强） |
| `x.pls` / `x.plsql` / `x.pkb` / `x.pks` | `plsql`（强） |
| `x.sql` | `plsql`（强，`.sql` 唯一归属） |
| `x.tsql` | 未知（T-SQL 已关闭）→ 提示走手动锁 |
| 打开 T-SQL 后的 `x.sql` | 歧义（`plsql` 与 `tsql` 都归属）→ 手动锁 |

---

## 8. 两个引擎的一致性

### 8.1 VS Code 桥接（JavaScript 引擎）

- 注册表同时持有「所有发现的插件包」与「已启用的插件包」。
- `resolveLanguage` 的扩展名解析由注册表驱动（§7）。
- 用户导入的包**只在这里可用**。

### 8.2 Windows 输入法 / Rust 核心

- Rust 核心把**发行版随附的**插件包在**编译期**编进二进制（`include_str!`），因此 Cypher / PL/SQL / T-SQL 在输入法里可用。
- Rust 核心**没有运行时包加载器**（见 §8.3）。
- 核心的「启用插件集合」是进程内状态，默认 `["cypher", "plsql"]`（与产品默认值一致），可由协议 op `setPluginLanguages` 改。
- 解析同样按 §7 由注册表驱动，因此**同一启用集合下，两个引擎的解析结果完全相同**。

### 8.3 已知限制（必须诚实标注，不得虚报）

1. **用户导入的插件包在 Windows 输入法里不可用。** Rust 核心没有运行时包加载器，包在编译期就固定了。
   - 输入法里可用的是发行版随附的那几个。
   - 用户导入的新语言在 VS Code 里立即可用。
   - 若要把一门新语言带进输入法，需要随发行版提供（在 `language-plugins/` 加包并重新编译核心）。
2. **插件语言没有符号索引规则。** F1 注释用的符号索引目前只认识内置 7 种语言；插件语言的符号列表为空，
   AI 注释因此拿不到该语言的符号。**不得**把「空列表」当成「没有符号」。
3. **候选校验强度有限。** SQL 族检查器是结构检查，不是编译器；「valid」只表示「这些检查没发现问题」。

---

## 9. 用户怎么用

### 9.1 打开 / 关闭插件语言

VS Code：命令面板 → **Programming IME: 插件语言**（勾选列表）→ 写入 `programmingIme.pluginLanguages`。

### 9.2 导入一个语言包

VS Code：命令面板 → **Programming IME: 导入语言插件** → 选一个文件夹 →
该文件夹里的每个 `*.json` 会被复制到 `<globalStorage>/language-plugins/` 并按 §4 校验；
通过的会被列出，失败的会逐个报出原因。**导入本身不会自动打开**该语言，还要在上面的列表里勾上。

### 9.3 看插件包放哪了

VS Code：命令面板 → **Programming IME: 打开插件文件夹**。

---

## 10. 作者视角：新增一门语言

1. 写一个包文件（§3），放进 `01_Source/language-plugins/`。
2. `validator` 选一个种类；没有现成种类就**新增一种**（同时改两个引擎，见 §11）。
3. 若需要新的扩展名：确认它没有被内置包占用。
4. 若需要方言版本轴：`values` 按从旧到新排。
5. 写测试（§11.3）。
6. 把包加进发行版（`language-plugins/` 整目录进包，`.vsix` 也要带）。

---

## 11. 变更规则

### 11.1 兼容性

- `schema_version` 仍然是 `1`。破坏性变更必须升到 `2`，并同时提供迁移路径；**不认识的版本直接拒绝**。
- 新增字段属于破坏性变更（老引擎会因「未知字段」拒绝该包）。

### 11.2 两侧必须同时改

新增一种**验证器种类**、新增一种**方言检查**、改动**扩展名解析规则**时，
JavaScript 引擎与 Rust 核心**必须同时改**，否则两个引擎对同一段代码的判断会不一致。

### 11.3 测试要求（缺一不可）

| 层 | 要求 |
|---|---|
| 账本层 | 包能被发现、能被列出、能被打开与关闭 |
| 行为层 | 已启用时：该 profile 能产出候选；已关闭时：一律无候选、不可锁 |
| 判据层 | SQL 族的检查码与行号由 `tests/fixtures/validate-cases.json` 对拍两个引擎 |

> 依据既有教训：**「三方一致」不等于「三方都对」**。对拍只证明两个引擎给出相同答案，
> 不能证明答案正确。因此共享夹具里必须同时有**存在性断言**（该报错的必须真的报了错）。

---

## 12. 参考

- 实现（JavaScript 引擎）：`editor-bridges/vscode/src/core/languages.js`、`core/validators.js`
- 实现（Rust 核心）：`crates/pime-core/src/pack.rs`、`validators.rs`、`resolve.rs`、`predict.rs`、`protocol.rs`
- 发行版随附的插件包：`01_Source/language-plugins/`
- 宿主设置：`programmingIme.pluginLanguages`
