# 广度优先搜索（Breadth-First Search）

- 资产 ID：`algorithm.graph.bfs`
- 类型：`algorithm`（算法）
- 分类：`graph`
- 版本：`1.0.0`
- 状态：`DOCUMENTED`（知识卡已成文，尚未经人工审阅；本资产没有任何已认证的语言实现）
- 别名：`bfs`、`广度优先搜索`、`breadth first search`

## 一句话定位

一层一层地向外扩展，先被访问到的节点一定离起点最近，因而能求出无权图的最短路。

社交关系里"二度人脉"、地图上"最近的车站"、状态空间里的最少步数，这些都需要按层找最近的可达点。

## 典型源文件形状

```text
bfs(start):
    dist = array of size V filled with -1
    parent = array of size V filled with -1
    queue = FIFO queue
    dist[start] = 0
    queue.push(start)
    while queue not empty:
        v = queue.pop_front()
        for u in adj[v]:
            if dist[u] == -1:
                dist[u] = dist[v] + 1
                parent[u] = v
                queue.push(u)
    return dist, parent
```

## 适用条件

- 无权图（每条边代价相同）上的最短路径
- 需要最短路径树或路径回溯（配合 parent 数组）
- 状态空间搜索（棋类、状态机、分层可达性）
- 二分图判定、连通分量、层级遍历

## 反例

- 边有权重且不同时——BFS 的"最少边数"不等于最短距离
- 图规模极大只需连通性——并查集更省内存
- 需要找所有路径而非最短路径——DFS 更直接
- 图是 DAG 且只需拓扑序——拓扑排序更贴切

## 并发风险

- BFS 本身是串行算法；并行 BFS（level-synchronous）在超大规模图上有效，但需要处理每个层的屏障同步
- 并行 BFS 的常见瓶颈是"最慢的那一层决定整体进度"，层内负载不均会造成大量空闲
- 只读图结构共享是并发安全的；若搜索过程中需要写访问标记，必须用每线程一份的标记或原子操作
- 把图表示为不可变的邻接表（`Arc<Vec<Vec<usize>>>`）便于多线程并行搜索

## 替代方案

- `algorithm.graph.dfs` — DFS 适合找路径存在性、连通分量与拓扑序，但不给无权最短路。
- `algorithm.graph.dijkstra` — 边有权重时用 Dijkstra 求真正的最短路。
- `algorithm.graph.union_find` — 只需要连通性与合并时用并查集，空间更省。

## 测试说明

- 与手工构造的小图预期结果对比（层数、距离、路径）
- 不连通图：断言不可达节点的距离为 -1 且不 panic
- 自环、平行边、环状图
- 路径回溯：从 parent 数组还原路径并断言与距离一致
- 大图性能测试：断言在预期规模内完成

## 实现注意

- 优先标准库/成熟库：图算法没有标准库实现，成熟 crate 有 `petgraph`（MIT/Apache-2.0），引入前需核查许可
- 复杂度：时间 O(V + E)，额外空间 O(V)（距离 + 父节点 + 队列）
- 稳定性：不适用
- 内存：邻接表 O(V + E)；稠密图用邻接矩阵会退化到 O(V²) 空间
- 溢出：距离用 `u32`/`usize`；超大图用 `u32` 更省且足够（层数不会超过节点数）
- 极端输入：空图、单节点、全连通、极长链（队列长度 1）、星形图（队列长度 O(V)）
- 并发安全：只读图上的搜索是线程安全的；写入访问标记需要原子或分片标记
- 限制：只对无权图给出最短路；大图的并行化需要专门的算法（如 level-synchronous BFS）

> 许可提醒：本卡正文为本仓库原创（MIT）；上列第三方链接仅作背景参考，其版权与许可需独立核查，不随本仓库的 MIT 授权一并授予。
