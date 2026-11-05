# Dijkstra 最短路径（Dijkstra Shortest Path）

- 资产 ID：`algorithm.graph.dijkstra`
- 类型：`algorithm`（算法）
- 分类：`graph`
- 版本：`1.0.0`
- 状态：`DOCUMENTED`（知识卡已成文，尚未经人工审阅；本资产没有任何已认证的语言实现）
- 别名：`Dijkstra`、`最短路径`、`single source shortest path`

## 一句话定位

每次确定当前距离最小的顶点并松弛其出边，用优先队列把"已确定的顶点"高效取出。

地图导航、单源路由、依赖代价最短路径，这些图的边权天然非负，需要一次算出到所有点的最短距离。

## 典型源文件形状

```text
dijkstra(graph, source):
    dist = array of size V filled with INF;  dist[source] = 0
    prev = array of size V filled with -1
    pq = min-priority queue of (dist, vertex)
    pq.push((0, source))
    while pq not empty:
        (d, v) = pq.pop_min()
        if d > dist[v]: continue          // 过期的堆条目
        for (u, w) in out_edges(v):
            if d + w < dist[u]:
                dist[u] = d + w
                prev[u] = v
                pq.push((dist[u], u))
    return dist, prev
```

## 适用条件

- 边权非负的单源最短路
- 需要到所有可达顶点的距离与前驱（可回溯路径）
- 图较大且稀疏（道路、路由、依赖代价）
- 边权是整数或可用整数缩放的场景

## 反例

- 存在负权边——必须换成 Bellman-Ford；Dijkstra 会给出错误结果
- 需要所有点对之间的最短路——用 Floyd-Warshall 或重复跑单源
- 图极小且边权简单——朴素 O(V²) 实现可能更简单更快
- 边权是浮点且需要严格精度——浮点比较与相等判断需要谨慎

## 并发风险

- Dijkstra 本身串行；并行版本（Delta-stepping、并行优先队列）需要专门设计
- 共享的优先队列是并行化的主要瓶颈，通常用分片队列或堆外优先队列解决
- 只读图上单线程 Dijkstra 是线程安全的
- 若把结果数组放在 `Arc<Mutex<..>>` 中共享，每步加锁会严重拖慢

## 替代方案

- `algorithm.graph.bellman_ford` — 有负权边时用 Bellman-Ford，代价是 O(V·E)。
- `algorithm.graph.bfs` — 无权图用 BFS 即可，O(V + E)。
- `algorithm.graph.floyd_warshall` — 需要所有点对最短路且 V 较小时用 Floyd-Warshall。

## 测试说明

- 负权边必须被拒绝（构造带负权边的图，断言返回明确错误）
- 不可达顶点：断言距离为 INF 且不 panic
- 路径重建：用 prev 回溯并断言路径总权重等于 dist
- 与朴素 O(V²) 实现在小图上交叉比较
- 边界：单节点、零权边、自环、平行边、浮点权重

## 实现注意

- 优先标准库/成熟库：`petgraph` 提供 Dijkstra，引入前需核查许可
- 复杂度：时间 O((V + E) log V)（二叉堆 + 邻接表），额外空间 O(V + E)；用斐波那契堆可到 O(E + V log V)
- 稳定性：不适用
- 内存：距离数组 O(V)、前驱 O(V)、优先队列最坏 O(E)（可能有重复条目）
- 溢出：`dist + w` 可能溢出整数；用 `u64`/`u128` 或对 INF 使用饱和加法；权重累加用足够宽的类型
- 极端输入：完全图、百万级稀疏图、权重极大或极小、零权边
- 并发安全：只读图上的算法线程安全；并行化需要并发优先队列
- 限制：非负权是硬前提；不适用于带负权的图；大图上优先队列的内存不可忽视

> 许可提醒：本卡正文为本仓库原创（MIT）；上列第三方链接仅作背景参考，其版权与许可需独立核查，不随本仓库的 MIT 授权一并授予。
