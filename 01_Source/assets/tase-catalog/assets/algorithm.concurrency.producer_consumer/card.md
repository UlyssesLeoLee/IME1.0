# 生产者–消费者（Producer-Consumer Problem）

- 资产 ID：`algorithm.concurrency.producer_consumer`
- 类型：`algorithm`（算法）
- 分类：`concurrency`
- 版本：`1.0.0`
- 状态：`DOCUMENTED`（知识卡已成文，尚未经人工审阅；本资产没有任何已认证的语言实现）
- 别名：`producer_consumer`、`生产者消费者`、`bounded buffer problem`

## 一句话定位

生产者往缓冲区写、消费者从缓冲区取，双方通过"缓冲区满/空"这一条件互相制约。

解码视频时，解码线程产生的帧比渲染线程消费得快，没有缓冲就会丢帧或阻塞。

## 典型源文件形状

```rust
use std::sync::{Arc, Condvar, Mutex};

pub struct BoundedBuffer<T> {
    queue: Mutex<VecDeque<T>>,
    not_empty: Condvar,
    not_full: Condvar,
    capacity: usize,
}

impl<T> BoundedBuffer<T> {
    pub fn push(&self, item: T) {
        let mut q = self.queue.lock().unwrap();
        while q.len() == self.capacity { q = self.not_full.wait(q).unwrap(); }
        q.push_back(item);
        self.not_empty.notify_one();
    }
}
```

## 适用条件

- 生产与消费的速率不同，需要解耦
- 多个生产者、多个消费者
- 需要背压来限制内存与延迟
- 阶段之间需要一定程度的并行

## 反例

- 数据量小且结构简单——直接顺序调用
- 需要严格顺序且不关心吞吐——单线程更简单
- 跨进程/跨机器——需要消息中间件而不是内存队列
- 消费者比生产者快且数据可丢弃——无界队列或直接丢弃更省事

## 并发风险

- 等待条件变量时必须用 `while` 循环重新检查条件（不能用 `if`），否则会丢失唤醒
- 通知用 `notify_one` 还是 `notify_all` 要按语义选：多消费者时 `notify_one` 更快但需防止"唤醒后条件已变"的空转
- 互斥锁只保护队列本身，不要在持锁期间执行 IO 或回调，否则会阻塞所有参与者
- 毒化（poisoned）的锁要在错误处理策略上明确：崩溃后是否允许继续运行
- 关闭时要保证"不会再有新元素"且"缓冲区里还有残留"，消费者要能区分"暂时空"与"已结束"
- 多个消费者时要注意公平性：简单的 FIFO 唤醒顺序可能让某些消费者饥饿

## 替代方案

- `algorithm.concurrency.bounded_queue` — 有界队列是生产者–消费者的标准实现载体，两者是同一问题的两个视角。
- `pattern.concurrent.pipeline` — 流水线是生产者–消费者的多阶段推广。

## 测试说明

- 断言元素不丢不重（生产 N 个、消费 N 个，用集合或计数校验）
- 断言缓冲区容量被真正限制（生产快于消费时内存不增长）
- 断言关闭后消费者能收到结束信号（不挂死）
- 断言条件变量用法正确（用 `while` 而非 `if`）——可通过压力测试暴露丢唤醒
- 多生产者多消费者下的正确性与公平性
- 毒化锁场景的行为符合约定

## 实现注意

- 优先标准库：Rust 的 `std::sync::mpsc::sync_channel(n)` 已封装了有界缓冲 + 条件变量；`crossbeam-channel` 提供更多选择，引入前需核查许可
- 复杂度：每次交接 O(1)（摊还），空间 O(缓冲区容量)
- 稳定性：不适用
- 内存：固定容量，内存占用可预测
- 溢出：容量用 `usize`；`VecDeque` 扩容受容量约束不会无限增长
- 极端输入：容量 1、生产者数量远大于消费者、消费者被长时间挂起、频繁关闭重开
- 并发安全：这是并发原语本身；错误在于锁/条件变量的误用（丢唤醒、持锁 IO、毒化锁）
- 限制：仅限单进程内存；跨进程需消息中间件；不支持多副本持久化

> 许可提醒：本卡正文为本仓库原创（MIT）；上列第三方链接仅作背景参考，其版权与许可需独立核查，不随本仓库的 MIT 授权一并授予。
