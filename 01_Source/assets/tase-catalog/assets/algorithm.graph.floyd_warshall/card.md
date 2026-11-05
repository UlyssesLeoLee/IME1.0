# Floyd-Warshall 全源最短路（Floyd-Warshall Algorithm）

- 资产 ID：`algorithm.graph.floyd_warshall`
- 类型：`algorithm`（算法）
- 分类：`graph`
- 版本：`1.0.0`
- 状态：`DOCUMENTED`（知识卡已成文，尚未经人工审阅；本资产没有任何已认证的语言实现）
- 别名：`floyd_warshall`、`FloydWarshall`、`弗洛伊德-沃肖尔`、`全源最短路`

## 一句话定位

三重循环的动态规划：允许每个顶点作为中转点，逐步把"直达"升级为"经中转更短"。

几十个节点之间的路由表需要一次性算全；或者要在若干候选节点中挑一个最优中转点。

## 典型源文件形状

```text
floyd_warshall(weight[V][V]):        // weight[i][j] = 边权，无边为 INF，对角为 0
    for k in 0..V:
        for i in 0..V:
            if weight[i][k] == INF: continue
            for j in 0..V:
                if weight[k][j] == INF: continue
                cand = weight[i][k] + weight[k][j]
                if cand < weight[i][j]:
                    weight[i][j] = cand
    return weight
```

## 适用条件

- V 较小（几百以内）且需要所有点对距离
- 需要用同一张距离矩阵回答多次查询
- 需要在 Floyd-Warshall 之前用 Johnson 算法处理负权边
- 密集图（E 接近 V²）时相对更划算

## 反例

- V 上千——O(V³) 不可接受
- 只需要单源或少数源——跑 Dijkstra
- 图稀疏且很大——重复跑单源算法更快
- 需要最短路径本身而不只是距离——需要额外的路径恢复矩阵，内存翻倍

## 并发风险

- 最外层的 k 循环存在顺序依赖，不能简单地并行化各轮
- 可以在同一轮 k 内并行化 i 与 j 的更新，但要注意 dist[i][k] 与 dist[k][j] 在本轮内不应被改变（经典实现需缓存）
- 分块并行（Floyd-Warshall 的 GPU 变体）是研究课题，工程上不必自研
- 只读的距离矩阵分块计算后合并需要额外的一致性处理

## 替代方案

- `algorithm.graph.dijkstra` — 单源或少量源用 Dijkstra，稀疏图上明显更快。
- `algorithm.graph.bellman_ford` — 有负权边且 V 小时可用 Bellman-Ford 重复跑，但 Floyd-Warshall 更直接。

## 测试说明

- 与对每对顶点跑 Dijkstra 的结果交叉比较
- 不可达点对：断言 INF 且不参与加法
- 负权边（无负环）：断言结果正确
- 负权环：断言检测到或明确说明不支持
- 边界：V = 0、1、2；自环；零权边

## 实现注意

- 优先标准库/成熟库：`petgraph` 提供 Floyd-Warshall，引入前需核查许可
- 复杂度：时间 O(V³)，额外空间 O(V²)（距离矩阵；带路径恢复再加同等量）
- 稳定性：不适用
- 内存：O(V²) 是主要瓶颈——V = 1000 时约 8 MB（u64），V = 10000 时 800 MB 不可行
- 溢出：`dist[i][k] + dist[k][j]` 会溢出，必须先判断两者是否为 INF
- 极端输入：稠密完全图、含负权边、含零权边、空图
- 并发安全：矩阵分块并行需要保证本轮不读写正在更新的 k 行/列（或做快照）
- 限制：O(V³) 时间与 O(V²) 空间，只适合小规模图

> 许可提醒：本卡正文为本仓库原创（MIT）；上列第三方链接仅作背景参考，其版权与许可需独立核查，不随本仓库的 MIT 授权一并授予。
