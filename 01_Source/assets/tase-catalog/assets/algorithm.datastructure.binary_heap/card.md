# 二叉堆（Binary Heap）

- 资产 ID：`algorithm.datastructure.binary_heap`
- 类型：`algorithm`（算法）
- 分类：`datastructure`
- 版本：`1.0.0`
- 状态：`DOCUMENTED`（知识卡已成文，尚未经人工审阅；本资产没有任何已认证的语言实现）
- 别名：`binary_heap`、`二叉堆`、`堆`、`heap`

## 一句话定位

数组实现的完全二叉树，父节点不小于子节点，于是堆顶就是全局最值。

任务调度要"每次拿优先级最高的任务"，维护一个有序数组会在插入时付出 O(n) 的移动代价。

## 典型源文件形状

```rust
// Rust 标准库已经提供 BinaryHeap（最大堆）
use std::collections::BinaryHeap;

let mut heap = BinaryHeap::new();
heap.push(3);
heap.push(5);
let top = heap.peek();          // O(1)
let popped = heap.pop();       // O(log n)，返回最大元素
```

## 适用条件

- 优先队列与任务调度
- Top-K / 流式最值维护
- Dijkstra 的辅助结构
- 需要反复取最大/最小且不要求整体有序

## 反例

- 需要全部元素有序——堆只保证堆顶有序
- 只需要一次最值且数据不变——一次 O(n) 扫描更简单
- 需要按范围或前缀查询——用有序结构或树
- 数据量很小——`Vec` 加排序更省

## 并发风险

- `BinaryHeap` 不是线程安全的共享结构；每个线程持有自己的堆最简单
- 需要共享时用 `Mutex<BinaryHeap>`，但每次 push/pop 都要加锁
- 生产者-消费者场景应把堆放在消费者线程内，通过通道交换任务，避免锁竞争
- 堆的 `Ord` 实现若比较的不是全序（例如浮点数含 NaN），堆的性质会被破坏

## 替代方案

- `algorithm.sorting.top_k_heap` — Top-K 是堆的典型应用形态。
- `algorithm.datastructure.monotonic_stack` — 单调栈用于"下一个更大元素"类问题，与堆的取最值不同。

## 测试说明

- 断言 pop 出来的元素是非递增的
- 插入后 peek 返回最大元素
- 空堆的 pop 返回 None 而不 panic
- 大量重复值、负数、极值
- 与 `sort` 的结果对比验证内部有序性
- 随机操作序列与参考实现（排序后取最大）交叉比较

## 实现注意

- 优先标准库：Rust 的 `std::collections::BinaryHeap` 已是成熟实现（最大堆，用 `Reverse` 可得最小堆），不要手写
- 复杂度：插入 O(log n)、弹出 O(log n)、取顶 O(1)、建堆 O(n)（自底向上）、空间 O(n)
- 稳定性：不适用（堆不是稳定结构）
- 内存：连续数组，O(n)；容量翻倍策略会有一时的额外占用
- 溢出：比较用 `Ord`；浮点数含 NaN 时比较不满足全序，堆性质不成立，需特殊处理
- 极端输入：空堆、单元素、全部相等、`i32::MIN`/`i64::MIN`
- 并发安全：非线程安全共享；用每线程一份或通道传递
- 限制：只保证堆顶有序；无按范围查询；`Ord` 必须是一致全序

> 许可提醒：本卡正文为本仓库原创（MIT）；上列第三方链接仅作背景参考，其版权与许可需独立核查，不随本仓库的 MIT 授权一并授予。
