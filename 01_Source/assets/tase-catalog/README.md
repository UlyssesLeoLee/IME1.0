# TASE 统一标准资产目录（机器可读数据）

本目录是 IDE1.0 / TASE v1.1 **统一标准资产目录**的机器可读数据：93 个资产（GoF 23 + 现代工程模式 20 + 经典算法 50）的
**知识卡**（`card.md`）、**资产清单**（`manifest.json`）与一份全量索引（`catalog.json`）。

> **本目录是设计资产清单，不是 93 个已认证实现。**
> 这里只有"某个资产是什么、什么时候用、什么时候别用、有什么风险"的知识；
> 本仓库目前**没有任何**已通过认证的语言实现，`implementations` 一律为空。
> 详见 [§4 状态语义](#4-状态语义为什么是-documented)。

## 1. 目录布局

```text
01_Source/assets/tase-catalog/
├── README.md                     # 本文件：格式、状态语义、ID 全表、与设计书的对照
├── catalog.json                  # 全量索引（93 条），机器可读
└── assets/
    └── <asset_id>/
        ├── manifest.json         # 资产清单（字段逐字对应设计书 §19.1）
        └── card.md               # 知识卡正文（8 个固定小节）
```

- `<asset_id>` 目录名与 manifest 的 `asset_id`、catalog 的 `id` 三者必须完全一致。
- 全部文件为 **UTF-8 无 BOM**，行尾 **LF**。

## 2. 文件格式：为什么用 JSON 而不是 YAML

**这是一处对设计书的有意偏离，需要显式记录。**

设计书 [03_Detailed_Design.md §19.1](../../../00_Documents/01_Detailed_Design/IDE1_TASE_Detailed_Design_v1.1.md) 给出的 manifest 参考形状是 **YAML**，
本目录实际落盘的是 **JSON**。

| | 设计书 §19.1（YAML） | 本目录（JSON） |
|---|---|---|
| 字段名 | `schema_version` / `asset_id` / `applicability.constraints` / … | **逐字相同**，一个字符都没改 |
| 嵌套结构 | 缩进表示层级 | 显式的 `{}` / `[]` |
| 多语言名称 | `name: { zh-CN: …, en-US: … }` | 同左，JSON 强制要求引号 |
| 布尔值 | `true` / `false` | 同左 |
| 空值 | `null` 或留空 | 统一写 `null`（本目录实际未使用） |

**偏离理由：**

1. 本仓库的 crate 与脚本是**零第三方依赖**（没有 `serde`、没有 `serde_yaml`、没有 YAML 解析器）。
   仓库里已有的 JSON 解析器是自带的 `pime-core::json`（严格 RFC 8259，带大小与深度上限）。
   YAML 则需要额外引入解析器，与"零依赖"原则冲突。
2. JSON 与 YAML 在**表达能力上是同一个集合**（本目录用到的子集两者都覆盖），
   因此换格式不会丢失任何信息，也不会改变任何字段语义。
3. JSON 对**键重复**天然敏感且可被严格拒绝（`pime-core::json` 直接拒绝重复键），
   这对"资产清单不得出现两个同名字段"的场景是优点。
4. 校验脚本用 Node 的内置 `JSON.parse` 即可，零依赖要求天然满足。

**回退方式：** 若后续决定改用 YAML，只需按 §19.1 的字段名做一次机械转换（字段名不变），
校验脚本中的 JSON 解析处换成 YAML 解析即可，目录结构与内容不需要改动。

## 3. manifest 字段说明（对照设计书 §19.1）

| 字段 | 类型 | 本目录的实际取值 | 说明 |
|---|---|---|---|
| `schema_version` | string | `"tase.asset.v1"` | 与设计书一致 |
| `asset_id` | string | 如 `algorithm.graph.dijkstra` | 与目录名、catalog 条目一致 |
| `version` | string | `"1.0.0"` | 资产版本（semver） |
| `kind` | string | `"pattern"` / `"algorithm"` | 设计书 §19.5 的 `kind` 枚举 |
| `name` | object | `{"zh-CN": …, "en-US": …}` | 两种语言的显示名，缺一不可 |
| `aliases` | string[] | 检索别名（含 §14.3 的同义名） | 别名**不新增资产条目**，见 §4.4 |
| `category` | string | 如 `graph` / `creational` / `integration` | 分类 |
| `status` | string | 一律 `"DOCUMENTED"` | 见 §4.1 |
| `intent` | string | 一句话：这个资产解决什么问题 | 设计书 §19.1 要求 |
| `applicability.constraints` | object | 该资产特有的约束键值对 | 如 `negative_edges: "forbidden"` |
| `applicability.counterexamples` | string[] | 至少一条 | 设计书 §14.2 要求"反例" |
| `applicability.alternatives` | string[] | 其它资产的 id | 每个 id 必须在 catalog 中真实存在 |
| `complexity.time` | string | 算法写真实复杂度；模式写 `"n/a"` | 模式不是算法 |
| `complexity.extra_space` | string | 同上 | |
| `implementations` | array | 一律 `[]` | 见 §4.2 |
| `provenance.references` | string[] | 公开权威来源 URL | 见 §4.5 |
| `provenance.license` | string | 一律 `"MIT"` | 本仓库许可；第三方来源许可需独立核查 |

### 3.1 知识卡（`card.md`）的 8 个固定小节

每张卡都包含下列小节，校验脚本逐条检查：

1. `## 一句话定位` —— 定位 + 解决的痛点
2. `## 典型源文件形状` —— 真实代码骨架（Rust 为主），算法卡给出算法伪码
3. `## 适用条件` —— 什么时候该用
4. `## 反例` —— 什么时候**不该**用
5. `## 并发风险` —— 线程安全、全局可变状态、锁顺序、幂等性
6. `## 替代方案` —— 指向本目录内其它资产 id
7. `## 测试说明` —— 确定性输入、边界、极端输入、性能基准要点
8. `## 实现注意` —— 复杂度、稳定性、内存、溢出、极端输入、并发安全及限制

## 4. 状态语义：为什么是 `DOCUMENTED`

### 4.1 `status` 一律 `DOCUMENTED`

设计书 [01_Requirements.md §14.4](../../../00_Documents/00_Requirements_and_Basic_Design/IDE1_TASE_Requirements_v1.1.md) 规定，知识卡有 `DOCUMENTED/REVIEWED` 两种状态，
`REVIEWED` 的前提是**知识卡已被人审阅过**。本目录的 93 张卡是本轮一次性产出的，
**没有经过人工评审**，因此写 `REVIEWED` 是不实的，一律写 `DOCUMENTED`。

设计书 §19.1 示例里的 `status: APPROVED_CARD` 是**另一种命名**（"卡已通过批准"），
但同一节也写明"此 Manifest 是设计示例，不等于已经存在获批准的实现"。
本目录统一采用需求书 §14.4 的命名空间（`DOCUMENTED/REVIEWED`），
以 `status` 取值 `DOCUMENTED` 表达"已成文、未审阅"。

### 4.2 `implementations` 一律为空

设计书 §14.4："`Agent catalog.search` 可发现知识卡，**只有存在目标语言获批准实现且匹配约束**，才能调用 `catalog.instantiate`"。

本仓库目前没有为任何一个资产提供语言实现（没有 Rust/Python/TypeScript 的模板与认证流程产物），
因此 `implementations` 一律为 `[]`。**写一条占位实现等于造假**——它会让 Agent 以为存在可用模板，
从而在 `catalog.instantiate` 阶段产生错误的预期。

同理，`implementations[].sha256` 这类字段在本目录**不出现**（数组为空，没有对象可挂字段）；
设计书"严禁打包发布占位 hash"的要求因此自动满足。

### 4.3 校验脚本的对应门禁

`01_Source/scripts/validate-tase-catalog.js` 会在任何一条 `implementations[].status` 为
`APPROVED` / `TESTED` / `IMPLEMENTED` 时直接报错（"本目录没有任何已认证的实现"），
并要求"状态不是 `APPROVED` 的实现必须带 `sha256`"。这条门禁就是为了防止占位实现混进目录。

### 4.4 别名不虚增资产数量

设计书 §14.3："同义算法名称统一为检索别名：`disjoint_set_union` 指向 `algorithm.graph.union_find`，
`binary_exponentiation` 指向 `algorithm.math.fast_power`；**不得通过别名虚增实际资产数量**。"

本目录的落地方式：

- 这两个同义名作为 `aliases` 挂在**目标资产**上，不为别名单独建条目；
- 因此 `catalog.json` 正好 **93** 条，不会因为别名变成 95 条；
- 校验脚本会检查：任何别名都不得等于某个资产 id，且**一个别名只能指向一个资产**
  （否则检索结果会有歧义——例如 `最小生成树` 同时属于 Kruskal 和 Prim 就必须拆开）。

### 4.5 许可

`provenance.license` 写 `"MIT"`，指**本卡正文与本仓库代码**的许可。
设计书在该字段后的注释要求"第三方来源许可独立核查"：每张引用了第三方资料的卡片末尾都有一行
`> 许可提醒：…第三方链接仅作背景参考，其版权与许可需独立核查，不随本仓库的 MIT 授权一并授予。`

## 5. 93 条资产 ID 全表

- **GoF 23**：`pattern.gof.<category>.<id>`，category ∈ `creational` / `structural` / `behavioral`（设计书 §19.4）
- **现代工程模式 20**：`pattern.arch.*` / `pattern.integration.*` / `pattern.resilience.*` / `pattern.concurrent.*`（设计书 §14.2）
- **经典算法 50**：`algorithm.<category>.<id>`，category 取设计书 §14.3 的 8 个分类

### GoF 23 — 创建型（5）　`pattern.gof.creational.*`

| # | 资产 ID | 名称（zh-CN / en-US） | 别名 | 状态 |
|---|---|---|---|---|
| 1 | `pattern.gof.creational.abstract_factory` | 抽象工厂 / Abstract Factory | `AbstractFactory`、`抽象工厂模式`、`abstract factory` | `DOCUMENTED` |
| 2 | `pattern.gof.creational.builder` | 生成器 / Builder | `Builder`、`生成器模式`、`builder pattern` | `DOCUMENTED` |
| 3 | `pattern.gof.creational.factory_method` | 工厂方法 / Factory Method | `FactoryMethod`、`工厂方法模式`、`factory method` | `DOCUMENTED` |
| 4 | `pattern.gof.creational.prototype` | 原型 / Prototype | `Prototype`、`原型模式`、`clone pattern` | `DOCUMENTED` |
| 5 | `pattern.gof.creational.singleton` | 单例 / Singleton | `Singleton`、`单例模式`、`单例` | `DOCUMENTED` |

### GoF 23 — 结构型（7）　`pattern.gof.structural.*`

| # | 资产 ID | 名称（zh-CN / en-US） | 别名 | 状态 |
|---|---|---|---|---|
| 1 | `pattern.gof.structural.adapter` | 适配器 / Adapter | `Adapter`、`适配器模式`、`adapter pattern` | `DOCUMENTED` |
| 2 | `pattern.gof.structural.bridge` | 桥接 / Bridge | `Bridge`、`桥接模式`、`bridge pattern` | `DOCUMENTED` |
| 3 | `pattern.gof.structural.composite` | 组合 / Composite | `Composite`、`组合模式`、`composite pattern` | `DOCUMENTED` |
| 4 | `pattern.gof.structural.decorator` | 装饰 / Decorator | `Decorator`、`装饰器模式`、`decorator pattern` | `DOCUMENTED` |
| 5 | `pattern.gof.structural.facade` | 外观 / Facade | `Facade`、`外观模式`、`facade pattern` | `DOCUMENTED` |
| 6 | `pattern.gof.structural.flyweight` | 享元 / Flyweight | `Flyweight`、`享元模式`、`flyweight pattern` | `DOCUMENTED` |
| 7 | `pattern.gof.structural.proxy` | 代理 / Proxy | `Proxy`、`代理模式`、`proxy pattern` | `DOCUMENTED` |

### GoF 23 — 行为型（11）　`pattern.gof.behavioral.*`

| # | 资产 ID | 名称（zh-CN / en-US） | 别名 | 状态 |
|---|---|---|---|---|
| 1 | `pattern.gof.behavioral.chain_of_responsibility` | 责任链 / Chain of Responsibility | `ChainOfResponsibility`、`责任链模式`、`chain of responsibility` | `DOCUMENTED` |
| 2 | `pattern.gof.behavioral.command` | 命令 / Command | `Command`、`命令模式`、`command pattern` | `DOCUMENTED` |
| 3 | `pattern.gof.behavioral.interpreter` | 解释器 / Interpreter | `Interpreter`、`解释器模式`、`interpreter pattern` | `DOCUMENTED` |
| 4 | `pattern.gof.behavioral.iterator` | 迭代器 / Iterator | `Iterator`、`迭代器模式`、`iterator pattern` | `DOCUMENTED` |
| 5 | `pattern.gof.behavioral.mediator` | 中介者 / Mediator | `Mediator`、`中介者模式`、`mediator pattern` | `DOCUMENTED` |
| 6 | `pattern.gof.behavioral.memento` | 备忘录 / Memento | `Memento`、`备忘录模式`、`memento pattern` | `DOCUMENTED` |
| 7 | `pattern.gof.behavioral.observer` | 观察者 / Observer | `Observer`、`观察者模式`、`observer pattern` | `DOCUMENTED` |
| 8 | `pattern.gof.behavioral.state` | 状态 / State | `State`、`状态模式`、`state pattern` | `DOCUMENTED` |
| 9 | `pattern.gof.behavioral.strategy` | 策略 / Strategy | `Strategy`、`策略模式`、`strategy pattern` | `DOCUMENTED` |
| 10 | `pattern.gof.behavioral.template_method` | 模板方法 / Template Method | `TemplateMethod`、`模板方法模式`、`template method` | `DOCUMENTED` |
| 11 | `pattern.gof.behavioral.visitor` | 访问者 / Visitor | `Visitor`、`访问者模式`、`visitor pattern` | `DOCUMENTED` |

### 现代工程模式 — 架构（7）　`pattern.arch.*`

| # | 资产 ID | 名称（zh-CN / en-US） | 别名 | 状态 |
|---|---|---|---|---|
| 1 | `pattern.arch.dependency_injection` | 依赖注入 / Dependency Injection | `DependencyInjection`、`依赖注入`、`DI`、`dependency injection` | `DOCUMENTED` |
| 2 | `pattern.arch.repository` | 仓储 / Repository | `Repository`、`仓储模式`、`repository pattern` | `DOCUMENTED` |
| 3 | `pattern.arch.unit_of_work` | 工作单元 / Unit of Work | `UnitOfWork`、`工作单元`、`unit of work` | `DOCUMENTED` |
| 4 | `pattern.arch.cqrs` | 命令查询职责分离 / CQRS | `CQRS`、`命令查询职责分离`、`command query responsibility segregation` | `DOCUMENTED` |
| 5 | `pattern.arch.event_sourcing` | 事件溯源 / Event Sourcing | `EventSourcing`、`事件溯源`、`event sourcing` | `DOCUMENTED` |
| 6 | `pattern.arch.clean_architecture` | 整洁架构 / Clean Architecture | `CleanArchitecture`、`整洁架构`、`clean architecture` | `DOCUMENTED` |
| 7 | `pattern.arch.hexagonal_architecture` | 六边形架构 / Hexagonal Architecture | `HexagonalArchitecture`、`六边形架构`、`ports and adapters`、`端口与适配器` | `DOCUMENTED` |

### 现代工程模式 — 集成（6）　`pattern.integration.*`

| # | 资产 ID | 名称（zh-CN / en-US） | 别名 | 状态 |
|---|---|---|---|---|
| 1 | `pattern.integration.saga` |  Saga（长事务编排） / Saga | `Saga`、`saga pattern`、`长事务`、`saga orchestration` | `DOCUMENTED` |
| 2 | `pattern.integration.transactional_outbox` | 事务发件箱 / Transactional Outbox | `TransactionalOutbox`、`事务发件箱`、`outbox pattern`、`发件箱模式` | `DOCUMENTED` |
| 3 | `pattern.integration.publish_subscribe` | 发布–订阅 / Publish–Subscribe | `PublishSubscribe`、`发布订阅`、`pub/sub`、`publish subscribe` | `DOCUMENTED` |
| 4 | `pattern.integration.idempotent_consumer` | 幂等消费者 / Idempotent Consumer | `IdempotentConsumer`、`幂等消费者`、`idempotent consumer`、`幂等消费` | `DOCUMENTED` |
| 5 | `pattern.integration.cache_aside` | 旁路缓存 / Cache-Aside | `CacheAside`、`旁路缓存`、`cache aside`、`缓存旁路` | `DOCUMENTED` |
| 6 | `pattern.integration.read_copy_update` | 读副本更新 / Read-Copy-Update | `ReadCopyUpdate`、`读副本更新`、`read copy update`、`RCU` | `DOCUMENTED` |

### 现代工程模式 — 韧性（4）　`pattern.resilience.*`

| # | 资产 ID | 名称（zh-CN / en-US） | 别名 | 状态 |
|---|---|---|---|---|
| 1 | `pattern.resilience.circuit_breaker` | 断路器 / Circuit Breaker | `CircuitBreaker`、`断路器`、`circuit breaker` | `DOCUMENTED` |
| 2 | `pattern.resilience.bulkhead` | 隔离舱 / Bulkhead | `Bulkhead`、`隔离舱`、`bulkhead pattern`、`舱壁隔离` | `DOCUMENTED` |
| 3 | `pattern.resilience.retry_with_exponential_backoff_and_jitter` | 带指数退避与抖动的重试 / Retry with Exponential Backoff and Jitter | `RetryWithBackoff`、`指数退避重试`、`exponential backoff jitter`、`重试退避` | `DOCUMENTED` |
| 4 | `pattern.resilience.rate_limiter` | 限流器 / Rate Limiter | `RateLimiter`、`限流`、`rate limiting`、`throttling` | `DOCUMENTED` |

### 现代工程模式 — 并发（3）　`pattern.concurrent.*`

| # | 资产 ID | 名称（zh-CN / en-US） | 别名 | 状态 |
|---|---|---|---|---|
| 1 | `pattern.concurrent.actor` | Actor / Actor | `Actor`、`Actor模型`、`actor model` | `DOCUMENTED` |
| 2 | `pattern.concurrent.pipeline` | 流水线 / Pipeline | `Pipeline`、`流水线`、`pipeline pattern`、`数据流水线` | `DOCUMENTED` |
| 3 | `pattern.concurrent.object_pool` | 对象池 / Object Pool | `ObjectPool`、`对象池`、`object pool`、`连接池` | `DOCUMENTED` |

### 经典算法 — 排序与选择（7）　`algorithm.sorting.*`

| # | 资产 ID | 名称（zh-CN / en-US） | 别名 | 状态 |
|---|---|---|---|---|
| 1 | `algorithm.sorting.merge_sort` | 归并排序 / Merge Sort | `merge_sort`、`归并排序`、`mergesort`、`归并` | `DOCUMENTED` |
| 2 | `algorithm.sorting.heap_sort` | 堆排序 / Heap Sort | `heap_sort`、`堆排序`、`heapsort` | `DOCUMENTED` |
| 3 | `algorithm.sorting.quick_sort` | 快速排序 / Quicksort | `quick_sort`、`快速排序`、`quicksort` | `DOCUMENTED` |
| 4 | `algorithm.sorting.quickselect` | 快速选择 / Quickselect | `quickselect`、`快速选择`、`nth_element`、`第 k 大` | `DOCUMENTED` |
| 5 | `algorithm.sorting.counting_sort` | 计数排序 / Counting Sort | `counting_sort`、`计数排序`、`计数` | `DOCUMENTED` |
| 6 | `algorithm.sorting.radix_sort` | 基数排序 / Radix Sort | `radix_sort`、`基数排序`、`LSD`、`MSD` | `DOCUMENTED` |
| 7 | `algorithm.sorting.top_k_heap` | 前 K 大（堆） / Top-K with a Heap | `top_k_heap`、`前k大`、`top k`、`小顶堆选前K` | `DOCUMENTED` |

### 经典算法 — 查找（5）　`algorithm.searching.*`

| # | 资产 ID | 名称（zh-CN / en-US） | 别名 | 状态 |
|---|---|---|---|---|
| 1 | `algorithm.searching.binary_search` | 二分查找 / Binary Search | `binary_search`、`二分查找`、`二分搜索`、`折半查找` | `DOCUMENTED` |
| 2 | `algorithm.searching.lower_bound` | 下界查找 / Lower Bound | `lower_bound`、`下界`、`partition_point`、`第一个大于等于` | `DOCUMENTED` |
| 3 | `algorithm.searching.upper_bound` | 上界查找 / Upper Bound | `upper_bound`、`上界`、`第一个大于`、`partition_point_rev` | `DOCUMENTED` |
| 4 | `algorithm.searching.two_pointers` | 双指针 / Two Pointers | `two_pointers`、`双指针`、`对撞指针`、`双指针法` | `DOCUMENTED` |
| 5 | `algorithm.searching.sliding_window` | 滑动窗口 / Sliding Window | `sliding_window`、`滑动窗口`、`滑窗` | `DOCUMENTED` |

### 经典算法 — 图（9）　`algorithm.graph.*`

| # | 资产 ID | 名称（zh-CN / en-US） | 别名 | 状态 |
|---|---|---|---|---|
| 1 | `algorithm.graph.bfs` | 广度优先搜索 / Breadth-First Search | `bfs`、`广度优先搜索`、`breadth first search` | `DOCUMENTED` |
| 2 | `algorithm.graph.dfs` | 深度优先搜索 / Depth-First Search | `dfs`、`深度优先搜索`、`depth first search` | `DOCUMENTED` |
| 3 | `algorithm.graph.topological_sort` | 拓扑排序 / Topological Sort | `topological_sort`、`拓扑排序`、`toposort`、`拓扑序` | `DOCUMENTED` |
| 4 | `algorithm.graph.dijkstra` | Dijkstra 最短路径 / Dijkstra Shortest Path | `Dijkstra`、`最短路径`、`single source shortest path` | `DOCUMENTED` |
| 5 | `algorithm.graph.bellman_ford` | Bellman-Ford 最短路径 / Bellman-Ford Algorithm | `bellman_ford`、`BellmanFord`、`贝尔曼-福特`、`负权最短路` | `DOCUMENTED` |
| 6 | `algorithm.graph.floyd_warshall` | Floyd-Warshall 全源最短路 / Floyd-Warshall Algorithm | `floyd_warshall`、`FloydWarshall`、`弗洛伊德-沃肖尔`、`全源最短路` | `DOCUMENTED` |
| 7 | `algorithm.graph.kruskal` | Kruskal 最小生成树 / Kruskal’s Minimum Spanning Tree | `Kruskal`、`最小生成树`、`MST`、`克鲁斯卡尔算法` | `DOCUMENTED` |
| 8 | `algorithm.graph.prim` | Prim 最小生成树 / Prim’s Minimum Spanning Tree | `Prim`、`普里姆算法`、`Prim 最小生成树` | `DOCUMENTED` |
| 9 | `algorithm.graph.union_find` | 并查集 / Disjoint Set Union (Union-Find) | `union_find`、`disjoint_set_union`、`并查集`、`不相交集合`、`DSU` | `DOCUMENTED` |

### 经典算法 — 字符串（6）　`algorithm.string.*`

| # | 资产 ID | 名称（zh-CN / en-US） | 别名 | 状态 |
|---|---|---|---|---|
| 1 | `algorithm.string.kmp` | KMP 字符串匹配 / Knuth–Morris–Pratt Algorithm | `KMP`、`Knuth-Morris-Pratt`、`前缀函数匹配` | `DOCUMENTED` |
| 2 | `algorithm.string.z_algorithm` | Z 算法 / Z Algorithm | `z_algorithm`、`Z算法`、`Z-function`、`扩展KMP` | `DOCUMENTED` |
| 3 | `algorithm.string.trie` | 字典树 / Trie (Prefix Tree) | `trie`、`字典树`、`前缀树`、`prefix tree` | `DOCUMENTED` |
| 4 | `algorithm.string.aho_corasick` | Aho-Corasick 多模式匹配 / Aho–Corasick Algorithm | `aho_corasick`、`AhoCorasick`、`AC自动机`、`多模式匹配` | `DOCUMENTED` |
| 5 | `algorithm.string.rabin_karp` | Rabin-Karp 字符串匹配 / Rabin–Karp Algorithm | `rabin_karp`、`RabinKarp`、`滚动哈希`、`rolling hash` | `DOCUMENTED` |
| 6 | `algorithm.string.edit_distance` | 编辑距离 / Edit Distance (Levenshtein) | `edit_distance`、`编辑距离`、`Levenshtein`、`levenshtein distance` | `DOCUMENTED` |

### 经典算法 — 动态规划（6）　`algorithm.dp.*`

| # | 资产 ID | 名称（zh-CN / en-US） | 别名 | 状态 |
|---|---|---|---|---|
| 1 | `algorithm.dp.knapsack_01` | 0-1 背包 / 0/1 Knapsack | `knapsack_01`、`0-1背包`、`背包问题`、`knapsack` | `DOCUMENTED` |
| 2 | `algorithm.dp.longest_increasing_subsequence` | 最长上升子序列 / Longest Increasing Subsequence | `longest_increasing_subsequence`、`LIS`、`最长上升子序列`、`最长递增子序列` | `DOCUMENTED` |
| 3 | `algorithm.dp.longest_common_subsequence` | 最长公共子序列 / Longest Common Subsequence | `longest_common_subsequence`、`LCS`、`最长公共子序列` | `DOCUMENTED` |
| 4 | `algorithm.dp.coin_change` | 找零（硬币兑换） / Coin Change | `coin_change`、`找零`、`硬币兑换`、`最少硬币数` | `DOCUMENTED` |
| 5 | `algorithm.dp.interval_dp` | 区间动态规划 / Interval Dynamic Programming | `interval_dp`、`区间DP`、`矩阵链乘法`、`区间调度` | `DOCUMENTED` |
| 6 | `algorithm.dp.bitmask_dp` | 位掩码动态规划 / Bitmask DP (Subset DP) | `bitmask_dp`、`位掩码DP`、`状压DP`、`subset dp` | `DOCUMENTED` |

### 经典算法 — 数据结构（6）　`algorithm.datastructure.*`

| # | 资产 ID | 名称（zh-CN / en-US） | 别名 | 状态 |
|---|---|---|---|---|
| 1 | `algorithm.datastructure.binary_heap` | 二叉堆 / Binary Heap | `binary_heap`、`二叉堆`、`堆`、`heap` | `DOCUMENTED` |
| 2 | `algorithm.datastructure.segment_tree` | 线段树 / Segment Tree | `segment_tree`、`线段树`、`区间树`、`segment tree` | `DOCUMENTED` |
| 3 | `algorithm.datastructure.fenwick_tree` | 树状数组 / Fenwick Tree (Binary Indexed Tree) | `fenwick_tree`、`树状数组`、`binary indexed tree`、`BIT` | `DOCUMENTED` |
| 4 | `algorithm.datastructure.lru_cache` | LRU 缓存 / LRU Cache | `lru_cache`、`LRU`、`最近最少使用缓存`、`least recently used` | `DOCUMENTED` |
| 5 | `algorithm.datastructure.hash_table` | 哈希表 / Hash Table | `hash_table`、`哈希表`、`散列表`、`hash map` | `DOCUMENTED` |
| 6 | `algorithm.datastructure.monotonic_stack` | 单调栈 / Monotonic Stack | `monotonic_stack`、`单调栈`、`monotonic deque`、`单调队列` | `DOCUMENTED` |

### 经典算法 — 数学/数值（6）　`algorithm.math.*`

| # | 资产 ID | 名称（zh-CN / en-US） | 别名 | 状态 |
|---|---|---|---|---|
| 1 | `algorithm.math.euclidean_gcd` | 欧几里得求最大公约数 / Euclidean GCD | `euclidean_gcd`、`gcd`、`最大公约数`、`辗转相除法` | `DOCUMENTED` |
| 2 | `algorithm.math.sieve_of_eratosthenes` | 埃氏筛 / Sieve of Eratosthenes | `sieve_of_eratosthenes`、`埃氏筛`、`筛法`、`prime sieve` | `DOCUMENTED` |
| 3 | `algorithm.math.fast_power` | 快速幂（平方求幂） / Exponentiation by Squaring | `fast_power`、`binary_exponentiation`、`快速幂`、`平方求幂` | `DOCUMENTED` |
| 4 | `algorithm.math.modular_exponentiation` | 模幂运算 / Modular Exponentiation | `modular_exponentiation`、`模幂`、`模快速幂`、`pow mod` | `DOCUMENTED` |
| 5 | `algorithm.math.extended_euclidean` | 扩展欧几里得算法 / Extended Euclidean Algorithm | `extended_euclidean`、`扩展欧几里得`、`exgcd`、`贝祖系数` | `DOCUMENTED` |
| 6 | `algorithm.math.reservoir_sampling` | 蓄水池抽样 / Reservoir Sampling | `reservoir_sampling`、`蓄水池抽样`、`水塘抽样`、`reservoir` | `DOCUMENTED` |

### 经典算法 — 并发/可靠性（5）　`algorithm.concurrency.*`

| # | 资产 ID | 名称（zh-CN / en-US） | 别名 | 状态 |
|---|---|---|---|---|
| 1 | `algorithm.concurrency.bounded_queue` | 有界队列 / Bounded Queue | `bounded_queue`、`有界队列`、`bounded buffer`、`环形缓冲` | `DOCUMENTED` |
| 2 | `algorithm.concurrency.producer_consumer` | 生产者–消费者 / Producer-Consumer Problem | `producer_consumer`、`生产者消费者`、`bounded buffer problem` | `DOCUMENTED` |
| 3 | `algorithm.concurrency.work_stealing` | 工作窃取 / Work Stealing | `work_stealing`、`工作窃取`、`work stealing scheduler`、`窃取调度` | `DOCUMENTED` |
| 4 | `algorithm.concurrency.exponential_backoff_jitter` | 指数退避与抖动 / Exponential Backoff with Jitter | `exponential_backoff_jitter`、`指数退避抖动`、`backoff jitter`、`退避算法` | `DOCUMENTED` |
| 5 | `algorithm.concurrency.token_bucket` | 令牌桶 / Token Bucket | `token_bucket`、`令牌桶`、`token bucket`、`令牌算法` | `DOCUMENTED` |

## 6. Rust 的 `tase-registry` 如何读取本目录

> **现状说明：** `crates/tase-registry/` 目前**尚未创建**（设计书 03_Detailed_Design.md §1.1 规划的
> `tase-registry/src/{manifest,loader,policy,query,digest,validator}.rs`）。
> 本目录是该 crate 将要消费的数据契约，下面给出的是**与本仓库现状一致的读取路径**，不是已实现的代码。

### 6.1 读取路径

| 步骤 | 做什么 | 落点 |
|---|---|---|
| 1 | 定位目录 | 随安装包分发，路径形如 `<install>/assets/tase-catalog`；也可由 `tase-registry` 的 `loader.rs` 相对 crate 根解析 |
| 2 | 读索引 | `catalog.json` → 得到 93 条 `{id, kind, category, name_zh, name_en, aliases, status, manifest, card}` |
| 3 | 按需读清单 | 对 `catalog.json` 里的 `manifest` 字段做相对路径拼接，读该资产的 `manifest.json` |
| 4 | 读知识卡 | `card` 字段 → `card.md`（`catalog.inspect` 返回给调用方或 Agent 的可读内容） |
| 5 | 校验 | 由 `validator.rs` 复刻本目录校验脚本的硬门禁；入库前还要算 `card_digest`（设计书 §19.5 的 `catalog_assets.card_digest`） |

**解析器：** 本仓库没有 `serde`，但已有自带的严格 JSON 解析器 `pime-core::json`：

```rust
use pime_core::json::{parse, Limits, Json};

let text = std::fs::read_to_string(dir.join("catalog.json"))?;
// Limits::DEFAULT 的 max_bytes = 1 MiB、max_depth = 64：
// 本目录的 catalog.json 约 43 KB、最大的 manifest 约 1.5 KB，默认上限足够。
let value: Json = parse(&text, Limits::DEFAULT)?;

let assets = value.get("assets").and_then(Json::as_array).ok_or("缺少 assets")?;
let count = value.get("count").and_then(Json::as_i64).unwrap_or(0);
assert_eq!(count as usize, assets.len(), "catalog 的 count 与条目数必须一致");
```

### 6.2 字段映射（catalog.json / manifest.json → Rust 结构）

| catalog.json | manifest.json | Rust 侧（设计书 §1.1 的 `tase-registry`） | 说明 |
|---|---|---|---|
| `schema_version` | `schema_version` | `schema_version: String` | 固定 `tase.asset.v1` |
| `catalog_version` | — | `catalog_version: Version` | 目录整体版本 |
| `count` | — | — | 必须等于 `assets.len()` |
| `assets[].id` | `asset_id` | `AssetId` | 三处（目录名 / catalog / manifest）必须一致 |
| `assets[].kind` | `kind` | `AssetKind::{Pattern, Algorithm}` | 设计书 §19.5 的 CHECK 约束 |
| `assets[].category` | `category` | `category: String` | |
| `assets[].name_zh` / `name_en` | `name["zh-CN"]` / `name["en-US"]` | `name: LocalizedName` | |
| `assets[].aliases` | `aliases` | `aliases: Vec<String>` | 仅检索用，不参与主键 |
| `assets[].status` | `status` | `card_status` | 设计书 §19.5 `catalog_assets.card_status` |
| `assets[].manifest` | — | 相对路径 | 用于懒加载清单 |
| `assets[].card` | — | 相对路径 | 用于懒加载知识卡 |
| — | `version` | `asset_version` | 设计书 §19.5 `catalog_assets.asset_version` |
| — | `intent` | `intent` | |
| — | `applicability.constraints` | `constraints` | 供 §19.3 的硬门禁逐条校验 |
| — | `applicability.counterexamples` | `counterexamples` | 随机器结果返回，防止 Agent 无条件套用 |
| — | `applicability.alternatives` | `alternatives: Vec<AssetId>` | 必须能在 catalog 中解析到 |
| — | `complexity` | `complexity` | 供 `recommend` 的技术评分 |
| — | `implementations` | `implementations: Vec<Implementation>` | 本目录恒为空；非空时每个元素须有 `implementation_id/language/version/status/engine/path/sha256/parameters_schema/tests_manifest` |
| — | `provenance.references` | `references: Vec<String>` | |
| — | `provenance.license` | `license_spdx` | 设计书 §19.5 `catalog_implementations.license_spdx` |

### 6.3 与 `catalog.instantiate` 的关系

设计书 §19.3：`instantiate` 的前置是 `find_approved_implementation(candidate, language, …)`。
本目录 `implementations` 为空 ⇒ **本目录的任何资产都不可被实例化**，
`catalog.search` / `catalog.inspect` 可以返回知识卡与反例，`catalog.instantiate` 必须返回
`UNAPPROVED_IMPLEMENTATION`（对应测试用例 TC-037）。

## 7. 与设计书的对照

| 设计书位置 | 要求 | 本目录的落地 |
|---|---|---|
| 01_Requirements.md §14.1 | GoF 23 全集（5 创建 / 7 结构 / 11 行为） | 23 条，ID 前缀 `pattern.gof.<category>.<id>`（§19.4） |
| 01_Requirements.md §14.2 | 现代工程模式首批 20 项，标明"适用条件、反例、并发风险、替代方案和系统边界" | 20 条；四类 namespace 固定；每卡含上述五类信息（"系统边界"写在"适用条件"小节内） |
| 01_Requirements.md §14.3 | 经典算法首批 50 项，8 个分类 | 50 条，ID 为 `algorithm.<category>.<id>`，category 与表格一致 |
| 01_Requirements.md §14.3 | 别名规则、不得虚增数量 | 见 §4.4；catalog 恒为 93 条 |
| 01_Requirements.md §14.3 | "优先标准库/成熟库，密码学必须用经过审查的密码库" | 写入算法卡的"实现注意"小节 |
| 01_Requirements.md §14.4 | 知识卡状态 `DOCUMENTED/REVIEWED`，实现另有状态机 | 见 §4.1–§4.3 |
| 03_Detailed_Design.md §19.1 | Asset Manifest 字段清单 | 字段名逐字照抄；仅把 YAML 换成 JSON（见 §2） |
| 03_Detailed_Design.md §19.4 | GoF 23 资产 ID 规则 | 完全一致 |
| 03_Detailed_Design.md §19.5 | 目录数据表 `catalog_assets` / `catalog_implementations` | 字段映射见 §6.2；本目录只提供资产侧数据 |
| 03_Detailed_Design.md §19.4 | GoF `singleton` 明确并发初始化与全局可变状态风险 | `pattern.gof.creational.singleton` 卡的"并发风险"小节 |
| 03_Detailed_Design.md §19.4 | GoF `observer` 明确订阅生命周期 | `pattern.gof.behavioral.observer` 卡的"并发风险"小节 |
| 03_Detailed_Design.md §19.4 | GoF `visitor` 明确扩展维度权衡 | `pattern.gof.behavioral.visitor` 卡的"替代方案"与"并发风险"小节 |
| 03_Detailed_Design.md §22 / TC-032~TC-037 | 目录注册、别名搜索、约束拒绝、未批准实现拒绝 | 校验脚本 `validate-tase-catalog.js` 逐条对应 |

## 8. 校验

```bash
cd 01_Source
node scripts/validate-tase-catalog.js                 # 默认校验 assets/tase-catalog
node scripts/validate-tase-catalog.js <其它目录>       # 校验别的目录（负向测试用）
```

退出码 `0` = 全过，`1` = 有错。脚本检查：

1. `catalog.json` 的 `count` 与 `assets` 长度一致且正好 93；GoF 23 / 现代模式 20 / 算法 50；
2. 每个资产的 `manifest.json` 存在、可解析、字段齐全、`schema_version` 为 `tase.asset.v1`、`asset_id` 与目录名及 catalog 条目一致；
3. `name` 同时有 `zh-CN` 与 `en-US`；`intent` 非空；`applicability.constraints/counterexamples/alternatives` 存在且 `counterexamples` 非空；
4. `alternatives` 引用的 id 都真实存在；别名不与任何资产 id 重复，且一个别名只指向一个资产；
5. `status` 只能是 `DOCUMENTED` 或 `REVIEWED`；任何 `implementations[].status` 不得是 `APPROVED/TESTED/IMPLEMENTED`；非 `APPROVED` 的实现必须带 `sha256`；
6. 算法类资产的 `complexity.time` / `complexity.extra_space` 不得是 `n/a`；模式类资产必须有 `card.md`；
7. `card.md` 存在、非空、包含规定的 8 个小节标题；现代工程模式（`pattern.*` 但非 `pattern.gof.*`）还必须写明"系统边界"（需求书 §14.2）；
8. `card.md` 里出现的 `algorithm.*` / `pattern.*` 资产 id 引用必须都存在；
9. 打印按 kind / namespace 的统计与缺失项清单。

## 9. 许可

本目录的卡正文、`catalog.json` 与 `manifest.json` 由本仓库原创，以 **MIT** 授权
（与仓库根目录 `LICENSE` 一致）。卡片中引用的 Wikipedia 条目、AWS 官方文档等第三方资料
仅作背景参考，其版权与许可**需独立核查**，不随本仓库的 MIT 授权一并授予。
