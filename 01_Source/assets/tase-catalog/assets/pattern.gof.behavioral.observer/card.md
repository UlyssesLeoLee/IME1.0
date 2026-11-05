# 观察者（Observer）

- 资产 ID：`pattern.gof.behavioral.observer`
- 类型：`pattern`（设计模式）
- 分类：`behavioral`
- 版本：`1.0.0`
- 状态：`DOCUMENTED`（知识卡已成文，尚未经人工审阅；本资产没有任何已认证的语言实现）
- 别名：`Observer`、`观察者模式`、`observer pattern`

## 一句话定位

让主题在状态变化时主动通知一组订阅者，订阅者无需轮询、主题无需知道订阅者是谁。

一个配置或文档变化后，多个界面和组件都要跟着刷新；不引入通知机制就只能到处手动调用刷新函数。

## 典型源文件形状

```rust
pub trait Observer { fn update(&mut self, value: &str); }

pub struct Subject { observers: Vec<Box<dyn Observer>> }

impl Subject {
    pub fn subscribe(&mut self, o: Box<dyn Observer>) -> SubscriptionId {
        self.observers.push(o);
        SubscriptionId(self.observers.len() - 1)
    }
    pub fn unsubscribe(&mut self, id: SubscriptionId) {
        self.observers.remove(id.0);
    }
    pub fn notify(&mut self, value: &str) {
        for o in self.observers.iter_mut() { o.update(value); }
    }
}
```

## 适用条件

- 一个状态变化会影响多个互不相知的组件
- 订阅者数量动态变化（按需订阅/退订）
- 需要把"通知"与"处理"解耦，避免循环依赖
- 同一事件的不同处理者行为差异大，不适合塞进一个统一接口

## 反例

- 订阅关系固定——直接调用更直白，少一层间接
- 只有一个订阅者——观察者模式退化成普通回调
- 需要事件溯源、历史回放时——观察者只处理"现在"，应使用事件溯源
- 跨进程/跨系统广播——观察者是进程内直接引用，应使用发布订阅

## 并发风险

- 订阅生命周期是首要风险：观察者必须先退订再被销毁，否则主题持有悬垂引用；Rust 里用 `Drop` 退订或让主题持有 `Weak`
- 通知时持有主题的可变锁，而观察者在回调里又订阅/退订，会造成死锁或借用冲突；应先复制订阅者列表再通知，或在通知时不持锁
- 观察者回调可能耗时或阻塞，会拖慢主题；耗时工作应转交后台任务
- 跨线程通知需要 `Send`/`Sync` 约束与正确的唤醒顺序，否则会出现"改了但没看到"
- 观察者回调若抛异常/panic，会中断其余观察者的通知；Rust 中要逐个 `catch_unwind` 或明确传播策略

## 替代方案

- `pattern.integration.publish_subscribe` — 发布订阅通过中间broker解耦、可跨进程；观察者是进程内主题直接持有订阅者。
- `pattern.gof.behavioral.mediator` — 观察者是"广播"，中介者是"协商与协调"；多方需要一致决策时用中介者。

## 测试说明

- 断言订阅后能收到通知、退订后收不到
- 断言通知顺序符合约定（通常是订阅顺序）
- 测试观察者在回调中退订或新增订阅不会导致崩溃或死锁
- 断言主题销毁后观察者不再被调用（生命周期回归）
- 测试观察者抛错/panic 时其余观察者的行为符合文档约定

## 实现注意

- 设计书明确要求：本卡必须写清"订阅生命周期"——先退订再析构，或用 `Weak` 打破环
- 弱引用（`Weak`）比强引用更安全，能自动避免"主题持有观察者、观察者又持有主题"的环
- 推送式通知要注意负载：主题不应把大对象反复复制给每个观察者
- 观察者模式与事件溯源的区别要向使用者讲清楚，否则容易误用

> 许可提醒：本卡正文为本仓库原创（MIT）；上列第三方链接仅作背景参考，其版权与许可需独立核查，不随本仓库的 MIT 授权一并授予。
