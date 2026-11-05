# 桥接（Bridge）

- 资产 ID：`pattern.gof.structural.bridge`
- 类型：`pattern`（设计模式）
- 分类：`structural`
- 版本：`1.0.0`
- 状态：`DOCUMENTED`（知识卡已成文，尚未经人工审阅；本资产没有任何已认证的语言实现）
- 别名：`Bridge`、`桥接模式`、`bridge pattern`

## 一句话定位

把"抽象层"和"实现层"拆成两组独立演化的类，让两个维度自由组合而不产生组合爆炸的继承树。

消息类型 × 传输方式、报表类型 × 导出格式，两个维度都在增长时，继承实现会得到 2×3×… 棵子树，每加一个维度就要改所有子类。

## 典型源文件形状

```rust
pub trait Sender { fn send(&self, body: &str); }

pub struct HttpSender { url: String }
impl Sender for HttpSender {
    fn send(&self, body: &str) { /* HTTP POST */ }
}

pub enum Message { Text(String), Binary(Vec<u8>) }

pub struct Alert { sender: Box<dyn Sender> }
impl Alert {
    pub fn notify(&self, m: &Message) {
        match m {
            Message::Text(t) => self.sender.send(t),
            Message::Binary(b) => self.sender.send(&format!("{b:?}")),
        }
    }
}
```

## 适用条件

- 存在两个互相独立、且都在持续增长的变体维度
- 组合空间是笛卡尔积，继承树会导致类的数量爆炸
- 两个维度的生命周期/变化频率不同，希望一方变更不影响另一方
- 平台无关 API 需要在多种操作系统/后端上复用同一套抽象

## 反例

- 只有一个维度在变——多写一层桥接只是增加间接
- 变体组合固定且很少——枚举 + `match` 更简洁且可穷尽检查
- 两个维度其实高度相关（总是成对出现）——那不是两个维度
- 团队对间接层不熟悉——桥接的收益要到变体数量上来后才显现，过早引入只会增加理解成本

## 并发风险

- 桥接实现通常持有一个 `Box<dyn Trait>`，该 trait 默认不满足 `Send`/`Sync`；要跨线程传递必须加约束
- 实现层若共享状态（如连接池），桥接对象应持 `Arc` 而不是各自复制
- 抽象层不应在方法内部加锁把实现层的并发细节泄漏出去；并发策略属于实现层
- 若实现层用 `&mut self` 的可变接口，抽象层就无法被并发共享，此时应改为内部可变性（`Mutex`）或分离读写接口

## 替代方案

- `pattern.gof.structural.adapter` — 当两个接口已经存在且不兼容、需要转换时用适配器；桥接是从一开始就把两维分开设计。
- `pattern.gof.behavioral.strategy` — 只有一个维度且要替换的是"算法"时用策略更轻。

## 测试说明

- 对每个抽象 × 实现的组合跑同一组契约测试（可用宏批量生成组合用例）
- 断言新增实现不需要修改抽象层（用编译期约束或代码审查规则保证）
- 测试实现层被替换为假实现时，抽象层行为不变
- 不要在同一测试里同时断言两层的内部细节，避免测试与实现耦合

## 实现注意

- 桥接常被误用为"接口 + 一个实现"的组合；真正的判据是"两个维度各自独立增长"
- 在 Rust 中桥接通常表现为"trait + `Box<dyn Trait>`"，与策略模式的形态接近，区别在于变化轴的数量
- 桥接的类数量 = 抽象数 × 实现数，若实现数固定为 1，桥接退化成一个普通接口
- 避免把桥接用成万能解耦：真正的解耦需求应该先由模块边界承担

> 许可提醒：本卡正文为本仓库原创（MIT）；上列第三方链接仅作背景参考，其版权与许可需独立核查，不随本仓库的 MIT 授权一并授予。
