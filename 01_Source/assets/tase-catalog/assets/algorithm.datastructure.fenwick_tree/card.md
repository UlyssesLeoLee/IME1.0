# 树状数组（Fenwick Tree (Binary Indexed Tree)）

- 资产 ID：`algorithm.datastructure.fenwick_tree`
- 类型：`algorithm`（算法）
- 分类：`datastructure`
- 版本：`1.0.0`
- 状态：`DOCUMENTED`（知识卡已成文，尚未经人工审阅；本资产没有任何已认证的语言实现）
- 别名：`fenwick_tree`、`树状数组`、`binary indexed tree`、`BIT`

## 一句话定位

按二进制最低位组织的前缀和结构：查询与更新都只走 log n 步，且空间只有 n。

要支持"单点加、区间求和"，线段树能用但常数与空间都高一倍，树状数组是更省的经典解。

## 典型源文件形状

```rust
pub struct Fenwick { tree: Vec<i64>, n: usize }

impl Fenwick {
    pub fn add(&mut self, mut i: usize, delta: i64) {   // 0 基
        i += 1;
        while i <= self.n {
            self.tree[i] += delta;
            i += i & i.wrapping_neg();      // i += lowbit(i)
        }
    }
    pub fn prefix_sum(&self, mut i: usize) -> i64 {    // [0, i)
        let mut s = 0;
        while i > 0 {
            s += self.tree[i];
            i -= i & i.wrapping_neg();
        }
        s
    }
}
```

## 适用条件

- 单点修改 + 前缀/区间求和
- 需要比线段树更小的空间与常数
- 其它可逆的前缀聚合（计数、异或等）
- 离线算法（逆序处理、差分）

## 反例

- 需要区间最值——树状数组无法做（无法用前缀相减）
- 需要区间更新——要用两个树状数组（差分技巧）或改用线段树
- 需要二维区间查询——二维树状数组实现复杂
- 数据需要频繁整体重建——直接重算前缀和数组可能更简单

## 并发风险

- 可变结构，不适合并发写；常见是构建后只读（`Arc`）并发查询
- 离线算法常用"倒序 + 逆操作"把写操作转成查询，从而可以只读共享
- 只读树状数组的 `prefix_sum` 是纯读取，多线程安全
- 若需要运行期动态更新，需每线程一份或加锁

## 替代方案

- `algorithm.datastructure.segment_tree` — 需要任意区间聚合或区间更新时用线段树。
- `algorithm.datastructure.hash_table` — 需要按键计数与范围查询的组合时，树状数组更高效。

## 测试说明

- 与暴力前缀和数组交叉比较
- 单点更新后前缀与区间和正确
- 负增量、多次更新叠加
- 边界：i = 0、i = n、查询空区间、全区间
- 建树（O(n) 线性建树）与逐点插入的结果一致
- n = 1 与 n = 0

## 实现注意

- 优先标准库/成熟库：没有标准库实现；成熟 crate 有 `fenwick`、`bitvec` 等，引入前需核查许可
- 复杂度：建树 O(n)，查询与更新 O(log n)，空间 O(n)
- 稳定性：不适用
- 内存：O(n)；`i64` 下 n = 10^6 约 8 MB，比线段树省一半
- 溢出：`lowbit` 用 `i & i.wrapping_neg()` 避免对 0 取负导致 panic；累加值可能溢出 `i64`
- 极端输入：n = 0、n = 1、最大索引、极大累加值
- 并发安全：只读共享安全；写需独占
- 限制：只支持可由前缀差分得到的聚合；不支持区间更新（需变通）

> 许可提醒：本卡正文为本仓库原创（MIT）；上列第三方链接仅作背景参考，其版权与许可需独立核查，不随本仓库的 MIT 授权一并授予。
