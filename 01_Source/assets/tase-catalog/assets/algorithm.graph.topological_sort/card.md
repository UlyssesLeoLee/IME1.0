# 拓扑排序（Topological Sort）

- 资产 ID：`algorithm.graph.topological_sort`
- 类型：`algorithm`（算法）
- 分类：`graph`
- 版本：`1.0.0`
- 状态：`DOCUMENTED`（知识卡已成文，尚未经人工审阅；本资产没有任何已认证的语言实现）
- 别名：`topological_sort`、`拓扑排序`、`toposort`、`拓扑序`

## 一句话定位

把"谁必须先于谁"的依赖关系排成一条线，且保证每个约束都被满足。

构建系统要先编译依赖再编译目标、任务调度要有先后、数据库迁移要按顺序执行——都是拓扑序问题。

## 典型源文件形状

```text
kahn_topological_sort(graph):
    in_degree = array of size V
    for v in V: in_degree[v] = number of incoming edges to v
    queue = all v with in_degree[v] == 0
    order = []
    while queue not empty:
        v = queue.pop_front()
        order.push(v)
        for u in out_edges(v):
            in_degree[u] -= 1
            if in_degree[u] == 0: queue.push(u)
    if len(order) != V: raise CycleError    // 有环
    return order
```

## 适用条件

- 构建/任务调度中的依赖顺序
- 模块导入顺序、数据管道阶段顺序
- 数据库迁移与版本升级顺序
- 课程/先修关系等层次依赖

## 反例

- 图中有环——必须先检测并处理，不能静默输出不完整结果
- 只需要任意合法顺序且图很小——DFS 后序更短
- 依赖关系是运行期动态确定的——静态拓扑排序不适用
- 存在大量等价顺序且需要"字典序最小"——需要更复杂的算法（如优先队列版本）

## 并发风险

- 拓扑排序本身是串行的；并行调度可以在拓扑约束下并发执行互不依赖的节点
- 构建系统的并行化正是基于拓扑序：同一层的任务可并发
- 并发构建时每个任务的产物需要独立目录或写时复制，避免互相覆盖
- Kahn 算法在共享图上只读是线程安全的；并行执行时要用依赖计数或就绪队列来调度

## 替代方案

- `algorithm.graph.dfs` — DFS 后序逆序也能得到拓扑序，且天然能报出环，但顺序不唯一。
- `algorithm.graph.bfs` — Kahn 算法本质上是按层剥离入度为 0 的节点，与 BFS 思路相近。

## 测试说明

- 断言输出满足所有依赖边（对每条边 u->v 断言 u 在 v 之前）
- 有环的图必须报错而不是给出不完整顺序
- 单节点、无边图、完全无序图、链式依赖
- 与 DFS 版本的结果对比：两者都是合法拓扑序（顺序可能不同）
- 空图返回空结果

## 实现注意

- 优先标准库/成熟库：`petgraph` 提供 `toposort`，引入前需核查许可
- 复杂度：时间 O(V + E)，额外空间 O(V)
- 稳定性：不适用（拓扑序一般不唯一）
- 内存：入度数组 + 队列 + 结果，O(V)
- 溢出：入度计数用 `usize`；并行边会让入度变大但不会溢出（除非边数极大）
- 极端输入：空图、单点、百万节点链、稠密 DAG
- 并发安全：只读图上的拓扑排序线程安全；并行执行拓扑序中的任务需要产物隔离
- 限制：只适用于 DAG；存在环时必须显式处理

> 许可提醒：本卡正文为本仓库原创（MIT）；上列第三方链接仅作背景参考，其版权与许可需独立核查，不随本仓库的 MIT 授权一并授予。
