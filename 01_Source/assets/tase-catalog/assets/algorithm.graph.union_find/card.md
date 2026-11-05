# 并查集（Disjoint Set Union (Union-Find)）

- 资产 ID：`algorithm.graph.union_find`
- 类型：`algorithm`（算法）
- 分类：`graph`
- 版本：`1.0.0`
- 状态：`DOCUMENTED`（知识卡已成文，尚未经人工审阅；本资产没有任何已认证的语言实现）
- 别名：`union_find`、`disjoint_set_union`、`并查集`、`不相交集合`、`DSU`

## 一句话定位

回答"这两个东西是不是一伙的"，并能把两伙合并到一起，几乎不花时间。

判断两个人是否在同一社交圈、判断若干集合是否可合并（最小生成树）、统计岛屿数量，都需要这种"合并并查询"的原语。

## 典型源文件形状

```rust
pub struct Dsu { parent: Vec<usize>, rank: Vec<u8> }

impl Dsu {
    pub fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]];   // 路径压缩（隔代压缩）
            x = self.parent[x];
        }
        x
    }
    pub fn union(&mut self, a: usize, b: usize) -> bool {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra == rb { return false; }
        match self.rank[ra].cmp(&self.rank[rb]) {
            Ordering::Less => self.parent[ra] = rb,
            Ordering::Greater => self.parent[rb] = ra,
            Ordering::Equal => { self.parent[rb] = ra; self.rank[ra] += 1; }
        }
        true
    }
}
```

## 适用条件

- 只需判断连通性与合并集合的场景（最小生成树、岛屿计数、社交圈）
- 元素是可索引的整数（0..n）
- 操作量大且频繁（需要近乎常数的摊还时间）
- 离线处理（所有操作已知）时可进一步用排序 + 扫描优化

## 反例

- 需要删除或拆分集合——并查集不支持，需要平衡树或链表合并等其它结构
- 需要遍历集合内的成员——并查集不直接支持（要额外维护成员列表）
- 元素是复杂对象且不方便索引——需要额外映射表
- 集合数量很小且查询极少——直接用数组或哈希表更省事

## 并发风险

- 并查集本身是可变结构，标准实现不是线程安全的
- 路径压缩会写父指针，多线程并发 find 会产生数据竞争（即使结果正确也会被 TSAN 报）
- 并发场景可改用"只读快照 + 分片合并"，或对每个分片加锁
- Rust 里可用 `Mutex<Dsu>` 串行化，但会失去并查集的性能优势；更好的做法是每个线程一份局部并查集
- 并查集的单调性（只会合并）使"批量离线 + 一次性合并"的并行策略天然可行

## 替代方案

- `algorithm.graph.kruskal` — Kruskal 用并查集判断加边是否成环，并查集是它的核心工具。
- `algorithm.graph.bfs` — 需要遍历与路径信息时用 BFS；并查集只回答"是否同集合"。

## 测试说明

- 断言初始时每个元素自成一集合
- 断言合并后同集合判定为真，且集合大小正确
- 断言合并一个已同集合的元素返回 false（不改变结构）
- 路径压缩后的查询仍返回正确根
- 随机操作序列与"朴素维护集合列表"的参考实现交叉比较
- 压力测试：百万次操作下确认摊还时间

## 实现注意

- 优先标准库/成熟库：Rust 标准库没有并查集，成熟 crate 有 `dsu`/`union-find`，引入前需核查许可
- 复杂度：时间 O(α(n)) 摊还（配合路径压缩 + 按秩/按大小合并），α 是反阿克曼函数，实际可视为常数
- 稳定性：不适用
- 内存：两个数组 O(n)
- 溢出：`rank` 用小整数即可；`size` 累加用 `usize`
- 极端输入：n = 0、全部合并成一个大集合、链式合并导致深度退化（路径压缩可解决）
- 并发安全：标准实现不是线程安全的；路径压缩写父指针存在数据竞争
- 限制：只支持合并与查询，不支持删除与拆分；元素必须是可索引的

> 许可提醒：本卡正文为本仓库原创（MIT）；上列第三方链接仅作背景参考，其版权与许可需独立核查，不随本仓库的 MIT 授权一并授予。
