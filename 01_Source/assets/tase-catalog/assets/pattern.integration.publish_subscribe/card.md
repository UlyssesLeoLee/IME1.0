# 发布–订阅（Publish–Subscribe）

- 资产 ID：`pattern.integration.publish_subscribe`
- 类型：`pattern`（设计模式）
- 分类：`integration`
- 版本：`1.0.0`
- 状态：`DOCUMENTED`（知识卡已成文，尚未经人工审阅；本资产没有任何已认证的语言实现）
- 别名：`PublishSubscribe`、`发布订阅`、`pub/sub`、`publish subscribe`

## 一句话定位

用 broker 把"发出事件"与"处理事件"解耦，让新的消费者能随时加入而不改发布方。

每加一个下游（通知、报表、审计）就要在业务代码里加一次调用，发布方代码随消费者数量膨胀。

## 典型源文件形状

```rust
pub trait EventBus: Send + Sync {
    fn publish(&self, topic: &str, key: &str, payload: &[u8]) -> Result<(), BusError>;
    fn subscribe(&self, topic: &str, handler: Box<dyn Handler>) -> Result<Subscription, BusError>;
}

pub struct OrderPlacedHandler;
impl Handler for OrderPlacedHandler {
    fn on_event(&self, event: &Event) { /* 幂等地处理 */ }
}
```

## 适用条件

- 一个事件需要被多个互不关心的下游消费
- 下游会持续增加，且不希望每次都改发布方
- 消费者可以异步处理，发布方不需要等待结果
- 需要在进程或服务之间解耦
- 系统边界：消息的路由与投递在 broker 的边界内，消息的业务语义与消费端的处理正确性在边界之外；顺序保证与保留策略需要另行约定。

## 反例

- 只有一个消费者且在同进程——直接调用即可
- 业务需要"发消息后立即确认成功"——同步调用语义更合适
- 消息很小且频率极高时——broker 的往返与序列化开销可能不划算
- 用发布订阅来隐藏"其实只是要调用一个函数"——那是把简单问题复杂化

## 并发风险

- 典型语义是至少一次：消费者必须幂等，否则重复投递会造成重复副作用
- 分区/队列决定并发度与顺序：同一业务键应固定到同一分区以保序
- 消费者组的再均衡（rebalance）会造成短暂重复与乱序，处理逻辑要能容忍
- broker 不可用时要决定行为：阻塞业务、降级丢弃还是本地缓冲（后者要配合发件箱）
- 死信队列要有监控与人工处置流程，否则失败消息会静默堆积

## 替代方案

- `pattern.gof.behavioral.observer` — 观察者是进程内直接引用的通知；发布订阅通过 broker 跨进程且支持持久化与重放。
- `pattern.integration.transactional_outbox` — 发布订阅解决消息流转，发件箱解决消息可靠产出，两者通常成对使用。

## 测试说明

- 消费者幂等测试：同一消息投递两次，断言副作用只发生一次
- 顺序测试：同一键的消息按序处理，跨键允许乱序
- 故障测试：broker 中断时，断言业务行为符合既定降级策略
- 重平衡测试：消费者增减时断言无消息丢失（最终都处理到）
- 端到端测试：发布 → 消费 → 断言下游状态更新

## 实现注意

- 事件契约（schema）必须版本化；下游对不认识的字段要能容忍
- 不要在消息里塞巨大的载荷，通常只放 id 与必要上下文，让消费者回查
- 发布订阅会带来"看不见的耦合"：事件字段变成事实上的公共 API
- 需要可观测性：每个 topic 的生产速率、消费延迟与失败率都要有指标

> 许可提醒：本卡正文为本仓库原创（MIT）；上列第三方链接仅作背景参考，其版权与许可需独立核查，不随本仓库的 MIT 授权一并授予。
