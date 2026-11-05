# 令牌桶（Token Bucket）

- 资产 ID：`algorithm.concurrency.token_bucket`
- 类型：`algorithm`（算法）
- 分类：`concurrency`
- 版本：`1.0.0`
- 状态：`DOCUMENTED`（知识卡已成文，尚未经人工审阅；本资产没有任何已认证的语言实现）
- 别名：`token_bucket`、`令牌桶`、`token bucket`、`令牌算法`

## 一句话定位

一个桶按固定速率自动添令牌，桶有容量上限；请求拿到令牌才能通过，没拿到就被限流。

既要限制长期平均速率（保护下游），又要允许短时突发（用户点一下刷新不该被拒），令牌桶正好同时表达这两点。

## 典型源文件形状

```rust
pub struct TokenBucket {
    capacity: f64,
    rate_per_sec: f64,
    state: Mutex<State>,
}

struct State { tokens: f64, last: Instant }

impl TokenBucket {
    pub fn try_acquire(&self, n: f64) -> bool {
        let mut s = self.state.lock().unwrap();
        let now = Instant::now();
        let elapsed = now.duration_since(s.last).as_secs_f64();
        s.tokens = (s.tokens + elapsed * self.rate_per_sec).min(self.capacity);
        s.last = now;
        if s.tokens >= n { s.tokens -= n; true } else { false }
    }
}
```

## 适用条件

- 需要限制平均速率但允许突发
- 保护下游服务与第三方 API
- 多租户系统里按租户分配配额
- 分布式限流（用共享存储实现令牌）

## 反例

- 不允许任何突发、要求平滑输出——用漏桶
- 只想限制并发请求数——用信号量或隔离舱
- 内部可信调用且不会过载——限流只是额外延迟
- 需要按权重分配（一次请求消耗多个令牌）——需扩展实现并防止绕过

## 并发风险

- 令牌数的"补充 + 判断 + 扣减"必须在一个临界区内完成，否则会超发
- 用 `Mutex<State>` 时不要在持锁期间做 IO；补充令牌是纯计算，持锁很短
- 时钟必须用单调时钟（`Instant`），墙钟回拨会导致令牌计算异常
- 分布式令牌桶要处理存储往返延迟与时钟漂移，常用"乐观扣减 + 失败放行"
- 多节点各自计数时总速率是节点数的倍数——需要共享存储或按节点数均分
- 令牌桶的拒绝判定应尽量在入口最靠前的位置做，避免先消耗资源

## 替代方案

- `pattern.resilience.rate_limiter` — 限流模式卡描述的是策略层面（维度、分布式、拒绝策略），本卡专注令牌桶这一具体算法。
- `algorithm.concurrency.bounded_queue` — 有界队列限制的是在途数量（并发），令牌桶限制的是速率。

## 测试说明

- 用可控时钟做确定性测试：推进时间后断言补充的令牌数正确
- 突发测试：桶满时连续放行不超过 capacity 个请求
- 速率测试：固定速率请求，断言长期放行数量符合速率
- 断言桶满后多余时间不会累积超量令牌（容量上限生效）
- 并发测试：多线程争抢令牌，断言放行总数不超过理论上限
- 边界：rate = 0、capacity = 0、请求消耗多个令牌

## 实现注意

- 优先标准库/成熟库：Rust 没有内置令牌桶；成熟 crate 有 `governor`（MIT/Apache-2.0），引入前需核查许可
- 复杂度：每次判定 O(1)，空间 O(1)
- 稳定性：不适用
- 内存：常数（每个桶几个数值）
- 溢出：令牌用浮点会累积误差，长期运行要定期校正；用整数微令牌（如毫秒粒度）可避免
- 极端输入：rate 极大、capacity 极大、时钟回拨（应改用单调时钟）、长时间空闲后突发
- 并发安全：需锁保护状态更新；分布式版本需处理存储延迟与时钟漂移
- 限制：只限平均速率，不直接限制并发；分布式实现依赖共享存储的可用性

> 许可提醒：本卡正文为本仓库原创（MIT）；上列第三方链接仅作背景参考，其版权与许可需独立核查，不随本仓库的 MIT 授权一并授予。
