# 前 K 大（堆）（Top-K with a Heap）

- 资产 ID：`algorithm.sorting.top_k_heap`
- 类型：`algorithm`（算法）
- 分类：`sorting`
- 版本：`1.0.0`
- 状态：`DOCUMENTED`（知识卡已成文，尚未经人工审阅；本资产没有任何已认证的语言实现）
- 别名：`top_k_heap`、`前k大`、`top k`、`小顶堆选前K`

## 一句话定位

维护一个大小为 k 的小顶堆：堆顶是"当前第 k 大"，比它小的直接丢弃，全程只花 O(n log k)。

从一亿条日志里找出最慢的 100 条，若整体排序要几十秒，而实际上只需要一个百来元素的候选集。

## 典型源文件形状

```rust
use std::collections::BinaryHeap;   // BinaryHeap 是最大堆，用 Reverse 变成最小堆
use std::cmp::Reverse;

pub fn top_k<T: Ord>(mut it: impl Iterator<Item = T>, k: usize) -> Vec<T> {
    let mut heap: BinaryHeap<Reverse<T>> = BinaryHeap::with_capacity(k);
    for item in it {
        if heap.len() < k {
            heap.push(Reverse(item));
        } else if item > heap.peek().unwrap().0 {
            heap.pop();
            heap.push(Reverse(item));
        }
    }
    heap.into_iter().map(|Reverse(v)| v).collect()
}
```

## 适用条件

- 数据以流的形式到达，无法全部载入内存
- 只需要前 k 个而不是完整排序
- k 远小于 n（经验上 k < n/10 时收益明显）
- 结果不需要全局有序（若需要，可再对 k 个元素排序）

## 反例

- n 很小——完整排序的常数更小
- k 与 n 同量级——堆的优势消失
- 需要精确的前 k 名且有并列——并列元素的取舍规则要显式定义
- 数据可多次查询且变化频繁——应考虑有序结构而不是每次重建堆

## 并发风险

- 堆本身不是线程安全的共享结构；每个线程维护自己的局部堆，最后合并是最简单的并行方案
- 并行归并各自的 top-k 结果时，合并方式要保证全局正确（取各分片结果的并集再选 k 个）
- 若堆放在 `Arc<Mutex<..>>` 中共享，每次 push/pop 都要加锁，高频场景会成为瓶颈
- 流式输入的读取与堆操作应解耦：用有界通道 + 单独的消费线程可避免锁竞争

## 替代方案

- `algorithm.sorting.quickselect` — 数据已全部在内存且只要第 k 个元素时，quickselect 更省内存。
- `algorithm.datastructure.binary_heap` — 本资产就是建立在二叉堆之上的用法，堆本身是数据结构资产。

## 测试说明

- 与"完整排序后取前 k"交叉比较，断言集合一致
- k = 0、k = 1、k = n（边界）
- 全部元素相同、含大量重复值
- 流式输入与一次性输入结果一致
- 并行分片后合并的结果与串行结果一致

## 实现注意

- 优先标准库：Rust 的 `BinaryHeap` 已是最大堆，前 k 大可用 `Reverse` 包装成最小堆
- 复杂度：时间 O(n log k)（每次比较 O(log k)），额外空间 O(k)
- 稳定性：不适用（只取集合）
- 内存：O(k)，与 n 无关，适合流式场景
- 溢出：k 与容量的乘法要检查；`BinaryHeap::with_capacity(k)` 的 k 很大时可能分配失败
- 极端输入：k = 0 时应返回空且不消耗输入；元素全等时堆的比较次数仍为 O(n log k)
- 并发安全：堆不是共享安全的；并行方案是每线程局部堆 + 归并
- 限制：需要能比较元素；若需要最终有序，对 k 个元素再排序即可（O(k log k)）

> 许可提醒：本卡正文为本仓库原创（MIT）；上列第三方链接仅作背景参考，其版权与许可需独立核查，不随本仓库的 MIT 授权一并授予。
