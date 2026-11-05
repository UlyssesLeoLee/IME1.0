# 适配器（Adapter）

- 资产 ID：`pattern.gof.structural.adapter`
- 类型：`pattern`（设计模式）
- 分类：`structural`
- 版本：`1.0.0`
- 状态：`DOCUMENTED`（知识卡已成文，尚未经人工审阅；本资产没有任何已认证的语言实现）
- 别名：`Adapter`、`适配器模式`、`adapter pattern`

## 一句话定位

在两种互不兼容的接口之间插一个薄转换层，让旧代码不用动就能接进新系统。

接入第三方 SDK 或遗留模块时，它的方法名、参数和错误类型都和我们的调用约定不同；改第三方不可能，改自己的调用点又会波及一大片代码。

## 典型源文件形状

```rust
// 第三方接口（不可修改）
pub struct LegacyClient { base: String }
impl LegacyClient {
    pub fn request(&self, path: &str) -> Result<String, String> {
        Ok(format!("{}:{}", self.base, path))
    }
}

// 我方期望的接口
pub trait HttpPort {
    fn get(&self, path: &str) -> Result<String, HttpError>;
}

pub struct LegacyAdapter { inner: LegacyClient }

impl HttpPort for LegacyAdapter {
    fn get(&self, path: &str) -> Result<String, HttpError> {
        self.inner.request(path).map_err(HttpError::Upstream)
    }
}
```

## 适用条件

- 第三方或遗留接口无法修改，但本系统需要统一的调用抽象
- 同一份业务代码要接入多个形态相近但接口不同的外部系统
- 单元测试需要用一个内存假实现替换真实外部依赖
- 隔离层能降低外部接口变更的影响面

## 反例

- 接口是我们自己控制的——直接改接口，别为了不面对重构而堆适配器
- 只是要做一次值转换——写一个 `convert()` 函数比一个类型更合适
- 两边语义并不等价（不是同一件事的不同说法）——适配器会掩盖模型差异，导致错误的数据被塞进目标模型
- 适配层里塞进业务逻辑——那是 Facade 或 Service 的职责

## 并发风险

- 适配器通常无状态，可自由跨线程共享；把它实现为 `Send + Sync` 才能放进 `Arc`
- 若被包装对象持有连接或句柄，不要把适配器做成全局单例来"省事"
- 适配层若缓存了转换结果，注意缓存的失效与并发写；最简单的做法是不缓存
- 错误转换要注意错误类型本身是否 `Send`——把携带非线程安全资源的错误跨线程抛出是常见崩溃源

## 替代方案

- `pattern.gof.structural.facade` — 当你要简化的是"多个子系统的调用"而不是"转换一个接口"时用外观。
- `pattern.gof.structural.bridge` — 当两个维度都要独立变化（接口与实现都可能多变）时用桥接。

## 测试说明

- 用契约测试保证适配器与被适配对象语义一致（同样的输入产生等价结果）
- 专门测错误路径：第三方返回错误时，适配器必须映射成本系统的错误类型，不能把原始错误字符串直接上抛
- 测试边界：空输入、超长路径、包含非 ASCII 的路径
- 若适配器是测试替身（fake），则测试 fake 与真实实现的一致性

## 实现注意

- Rust 中优先用 trait + `impl`（组合式适配器），少用继承式适配器；Rust 没有类继承
- 对象安全（object safety）决定了目标 trait 的方法不能有泛型参数或 `Self: Sized` 约束
- 适配器是防腐层（ACL），其边界越窄越好；不要在适配器里加业务规则
- 错误映射要保留上下文（用 `thiserror`/自定义错误枚举），不要退化成字符串

> 许可提醒：本卡正文为本仓库原创（MIT）；上列第三方链接仅作背景参考，其版权与许可需独立核查，不随本仓库的 MIT 授权一并授予。
