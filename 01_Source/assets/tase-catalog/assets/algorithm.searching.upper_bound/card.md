# 上界查找（Upper Bound）

- 资产 ID：`algorithm.searching.upper_bound`
- 类型：`algorithm`（算法）
- 分类：`searching`
- 版本：`1.0.0`
- 状态：`DOCUMENTED`（知识卡已成文，尚未经人工审阅；本资产没有任何已认证的语言实现）
- 别名：`upper_bound`、`上界`、`第一个大于`、`partition_point_rev`

## 一句话定位

二分查找的边界变体：返回第一个严格大于 x 的下标，正好框出所有等于 x 的元素区间。

统计某个时间戳的所有记录、删除某个键的全部条目，都需要知道"相等元素的右边界在哪"。

## 典型源文件形状

```rust
pub fn upper_bound<T: Ord>(a: &[T], x: &T) -> usize {
    let mut lo = 0usize;
    let mut hi = a.len();
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if a[mid] <= x { lo = mid + 1 } else { hi = mid }
    }
    lo                            // 第一个 > x 的位置
}
```

## 适用条件

- 需要等价区间的右端点（批量删除、批量计数、范围查询）
- 需要"第一个大于阈值的位置"
- 与 lower_bound 配合可切出子区间
- 处理有序数据中的重复元素

## 反例

- 只需要存在性——用 binary_search
- 序列无序——前提不满足
- 只需要单个元素——upper_bound 再减一即可，不需要两个都算
- 数据量小——线性扫描即可

## 并发风险

- 只读操作，共享不可变切片是并发安全的
- 查找期间数据必须保持有序且不变
- 并发删除时"查找区间 + 删除"需要原子性，通常在外层加锁
- 在有分片的存储上做 upper_bound 只能保证分片边界内的结果

## 替代方案

- `algorithm.searching.lower_bound` — lower_bound 给左端点，upper_bound 给右端点，两者配对定位等价区间。
- `algorithm.searching.binary_search` — 只需要命中位置时用 binary_search。

## 测试说明

- 与线性实现交叉比较，断言位置一致
- x 小于所有元素（0）、大于所有元素（len）、恰好等于所有元素（len）
- 空数组（0）
- 重复元素时断言返回最右边界
- 与 lower_bound 组合测试：断言区间 [lower, upper) 内元素全部等于 x

## 实现注意

- 优先标准库：Rust 标准库没有直接的 `upper_bound`，但可用 `partition_point(|x| x <= target)` 表达
- 复杂度：时间 O(log n)，额外空间 O(1)
- 稳定性：不适用
- 内存：O(1)
- 溢出：中点用 `lo + (hi - lo) / 2`
- 极端输入：空数组、全部相等时返回 len（调用方需处理该边界）
- 并发安全：只读安全，前提是数据不变
- 限制：要求有序；返回的区间端点语义要与调用方约定一致

> 许可提醒：本卡正文为本仓库原创（MIT）；上列第三方链接仅作背景参考，其版权与许可需独立核查，不随本仓库的 MIT 授权一并授予。
