# 享元（Flyweight）

- 资产 ID：`pattern.gof.structural.flyweight`
- 类型：`pattern`（设计模式）
- 分类：`structural`
- 版本：`1.0.0`
- 状态：`DOCUMENTED`（知识卡已成文，尚未经人工审阅；本资产没有任何已认证的语言实现）
- 别名：`Flyweight`、`享元模式`、`flyweight pattern`

## 一句话定位

把"成千上万个对象里真正重复的那部分状态"抽出来共享一份，让对象本身只携带随调用变化的部分。

地图上百万个地标、文本编辑器里几十万个字符样式，如果每个对象都独立持有字体度量与样式表，内存会爆炸。

## 典型源文件形状

```rust
use std::collections::HashMap;
use std::sync::Arc;

#[derive(PartialEq, Eq, Hash, Clone)]
pub struct StyleKey { pub font: String, pub size: u32 }

pub struct StyleSheet { entries: HashMap<StyleKey, Arc<StyleData>> }
impl StyleSheet {
    pub fn get(&mut self, key: &StyleKey) -> Arc<StyleData> {
        self.entries
            .entry(key.clone())
            .or_insert_with(|| Arc::new(StyleData::compile(key)))
            .clone()
    }
}
```

## 适用条件

- 对象数量极大且大部分状态重复（字符样式、图元、瓦片、AST 节点模板）
- 内部状态可被设计为不可变，从而安全共享
- 调用方愿意把随调用变化的状态作为外部参数传入
- 内存占用已被测量为瓶颈（先测量再优化，不要凭直觉引入）

## 反例

- 对象数量不大——直接创建更简单
- 对象状态几乎各不相同（外部状态占比高）——共享不到什么，反而增加一次查表
- 内部状态必须可变——共享同一份可变状态会产生跨对象污染
- 性能问题不在内存而在算法复杂度——享元解决不了

## 并发风险

- 共享的内部状态必须不可变；一旦可变，共享它的所有对象会互相影响
- 工厂（保存共享实例的容器）通常需要加锁：`RwLock<HashMap<..>>` 或 `Mutex`，读写比高时用 `RwLock`
- 用 `Arc` 共享内部状态，引用计数是原子的；注意 `Arc` 只保证生命周期，不保证内容安全
- `DashMap` 等并发哈希表可降低锁粒度，但引入额外依赖，与本仓库零依赖原则冲突时需评估
- 若缓存无上限，长期运行会持续增长内存——需要淘汰策略时参考 LRU 缓存资产

## 替代方案

- `pattern.gof.structural.adapter` — 适配器解决接口不兼容，与共享内存无关；两者常在同一处代码里并存但职责不同。
- `pattern.concurrent.object_pool` — 对象池复用的是"实例"（避免重新构造），享元共享的是"不可变数据"；池不减少唯一实例数。

## 测试说明

- 断言相同 key 的请求返回同一实例（`Arc::ptr_eq`），不同 key 返回不同实例
- 断言共享实例的内部状态在多次调用后没有被调用方意外修改（不可变性回归测试）
- 统计缓存命中与未命中次数，验证共享确实发生
- 在并发场景下测工厂：多线程请求相同 key，断言只创建一份且无数据竞争

## 实现注意

- 享元是内存优化，不是对象建模手段；先有内存剖面数据再引入
- Rust 中用 `&str`（借用）替代 `String`（拥有）常常就能达到类似效果，且更简单
- 享元与字符串驻留（interning）是同一思路的不同尺度
- 外部状态必须由调用方显式传入，不要偷偷塞进共享对象里，否则又变成共享可变状态

> 许可提醒：本卡正文为本仓库原创（MIT）；上列第三方链接仅作背景参考，其版权与许可需独立核查，不随本仓库的 MIT 授权一并授予。
