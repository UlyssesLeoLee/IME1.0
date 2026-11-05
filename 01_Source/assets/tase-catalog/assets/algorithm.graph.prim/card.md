# Prim 最小生成树（Prim’s Minimum Spanning Tree）

- 资产 ID：`algorithm.graph.prim`
- 类型：`algorithm`（算法）
- 分类：`graph`
- 版本：`1.0.0`
- 状态：`DOCUMENTED`（知识卡已成文，尚未经人工审阅；本资产没有任何已认证的语言实现）
- 别名：`Prim`、`普里姆算法`、`Prim 最小生成树`

## 一句话定位

像滚雪球一样从任一点开始，每次挑一条最便宜的"接到当前集合上"的边。

和 Kruskal 解决同一问题，但当图很稠密时，Prim 只需维护"每个顶点到树的最小边"，内存占用更小。

## 典型源文件形状

```text
prim(graph, start):
    in_tree = array of size V filled with false
    best = array of size V filled with INF     // 到已生成树的最小边权
    best[start] = 0
    pq = min-priority queue of (best[v], v)
    tree = []
    while pq not empty:
        (w, v) = pq.pop_min()
        if in_tree[v]: continue
        in_tree[v] = true
        if v != start: tree.push((prev[v], v, w))
        for (u, weight) in adj[v]:
            if not in_tree[u] and weight < best[u]:
                best[u] = weight
                prev[u] = v
                pq.push((weight, u))
    return tree
```

## 适用条件

- 无向带权图求最小生成树，且图较稠密
- 需要边集小、内存受限的场景（优先队列实现只需 O(V) 辅助空间）
- 需要增量式地"从某个点扩展出骨架"
- 网络设计、聚类、图像分割

## 反例

- 图很稀疏——Kruskal 更简单且边排序成本低
- 图是有向的——无定义
- 只需要最小生成森林且图不连通——需要显式处理每个连通分量
- 权重是负数——Prim 仍然正确（与 Dijkstra 不同，Prim 不要求非负）

## 并发风险

- Prim 的核心是优先队列，串行实现最简单；并行化需要并发优先队列
- 只读图上的 Prim 是线程安全的；多棵 Prim 同时跑同一图（不同起点）可并行
- 共享的 best 数组若被多线程更新需要原子最小值更新
- 并行 Kruskal 通常比并行 Prim 更容易实现

## 替代方案

- `algorithm.graph.kruskal` — 稀疏图用 Kruskal，两者结果的总权重相同（最小生成树唯一时边集也唯一）。
- `algorithm.graph.union_find` — 并查集是 Kruskal 的工具，Prim 不需要它。

## 测试说明

- 断言树有 V-1 条边、覆盖所有顶点、无环
- 断言总权重与 Kruskal 的结果一致
- 不连通图：断言明确处理
- 含平行边、自环、权重相等的边
- 与朴素 O(V²) 实现交叉比较

## 实现注意

- 优先标准库/成熟库：`petgraph` 提供 Prim，引入前需核查许可
- 复杂度：时间 O(E log V)（二叉堆）或 O(V²)（邻接矩阵），额外空间 O(V)
- 稳定性：不适用
- 内存：堆实现 O(V)；矩阵实现 O(V²)
- 溢出：总权重累加用足够宽的类型；INF 参与比较要正确初始化
- 极端输入：单节点、两点、多平行边、权重全相等、稠密完全图
- 并发安全：只读图安全；并行化需并发优先队列或分片
- 限制：只适用于无向图；与 Kruskal 一样，最小生成树不是最短路径树

> 许可提醒：本卡正文为本仓库原创（MIT）；上列第三方链接仅作背景参考，其版权与许可需独立核查，不随本仓库的 MIT 授权一并授予。
