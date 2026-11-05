# Bellman-Ford 最短路径（Bellman-Ford Algorithm）

- 资产 ID：`algorithm.graph.bellman_ford`
- 类型：`algorithm`（算法）
- 分类：`graph`
- 版本：`1.0.0`
- 状态：`DOCUMENTED`（知识卡已成文，尚未经人工审阅；本资产没有任何已认证的语言实现）
- 别名：`bellman_ford`、`BellmanFord`、`贝尔曼-福特`、`负权最短路`

## 一句话定位

反复对所有边做松弛，最多 V-1 轮；还能顺带发现负权环。

有些图确实存在负权边（例如"退款"是负的折扣），此时 Dijkstra 会给出错误答案，必须换成能处理负权的算法。

## 典型源文件形状

```text
bellman_ford(graph, source):
    dist = array of size V filled with INF;  dist[source] = 0
    prev = array of size V filled with -1
    for i in 1..=V-1:
        changed = false
        for (u, v, w) in edges:
            if dist[u] != INF and dist[u] + w < dist[v]:
                dist[v] = dist[u] + w
                prev[v] = u
                changed = true
        if not changed: break            // 提前收敛
    for (u, v, w) in edges:            // 负权环检测
        if dist[u] != INF and dist[u] + w < dist[v]:
            raise NegativeCycleError
    return dist, prev
```

## 适用条件

- 边权可能为负的单源最短路
- 需要检测负权环（套利、最短路不存在）
- 图规模不大或结构特殊（如分层图可用 SPFA 优化）
- 需要处理"代价可抵消"的建模（如折扣、退款）

## 反例

- 所有边权非负——用 Dijkstra
- 需要所有点对最短路且 V 小——用 Floyd-Warshall
- 图很大——O(V·E) 代价过高，应先考虑是否能重整模型使权重非负
- 用势能法（Johnson 算法）可以先消除负权边再跑 Dijkstra

## 并发风险

- 朴素 Bellman-Ford 是串行的；SPFA 等变体在某些图上更快但最坏仍是 O(V·E)
- 并行松弛：可以按边分组并行更新，但需要处理同一顶点的写冲突（用原子最小值或分片）
- 只读图上的多轮松弛是线程安全的，但共享 dist 数组时需要原子更新
- 稀疏图上可用队列式（SPFA）变体减少无效松弛

## 替代方案

- `algorithm.graph.dijkstra` — 权重非负时优先 Dijkstra。
- `algorithm.graph.floyd_warshall` — 需要所有点对时用 Floyd-Warshall。

## 测试说明

- 带负权边的图：断言结果与朴素实现一致
- 负权环：断言明确报错而不是返回错误距离
- 不可达顶点：断言 INF 处理正确
- 提前收敛：断言在无更新时能提前退出
- 路径重建：断言 prev 链可达起点且权重总和正确

## 实现注意

- 优先标准库/成熟库：`petgraph` 提供 Bellman-Ford，引入前需核查许可
- 复杂度：时间 O(V * E)（提前收敛时可更少），额外空间 O(V)
- 稳定性：不适用
- 内存：距离与前驱数组 O(V)；边表 O(E)
- 溢出：负权会让距离变小，`dist[u] + w` 可能下溢（用有符号类型并注意下界）；INF 参与加法必须先判 INF
- 极端输入：负权环、完全图、单点、零权边、极大权重
- 并发安全：只读图安全；并行松弛需要原子最小值更新
- 限制：O(V·E) 对大图不可行；负权环存在时最短路无定义

> 许可提醒：本卡正文为本仓库原创（MIT）；上列第三方链接仅作背景参考，其版权与许可需独立核查，不随本仓库的 MIT 授权一并授予。
