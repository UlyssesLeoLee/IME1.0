# 工厂方法（Factory Method）

- 资产 ID：`pattern.gof.creational.factory_method`
- 类型：`pattern`（设计模式）
- 分类：`creational`
- 版本：`1.0.0`
- 状态：`DOCUMENTED`（知识卡已成文，尚未经人工审阅；本资产没有任何已认证的语言实现）
- 别名：`FactoryMethod`、`工厂方法模式`、`factory method`

## 一句话定位

把"实例化哪个具体类"这一处决策收敛到一个可替换的方法里，调用方永远拿抽象类型。

`if cfg == "windows" { WindowsThing } else { UnixThing }` 散落在十几个调用点时，每加一种实现都要全量搜索修改。

## 典型源文件形状

```rust
pub trait Logger {
    fn log(&self, msg: &str);
    fn new_boxed() -> Box<dyn Logger> where Self: Sized;
}

pub struct FileLogger { path: std::path::PathBuf }

impl Logger for FileLogger {
    fn log(&self, msg: &str) { /* 写文件 */ }
    fn new_boxed() -> Box<dyn Logger> { Box::new(FileLogger {
        path: std::path::PathBuf::from("app.log"),
    }) }
}

pub struct StdoutLogger;
impl Logger for StdoutLogger {
    fn log(&self, msg: &str) { println!("{msg}"); }
    fn new_boxed() -> Box<dyn Logger> { Box::new(StdoutLogger) }
}
```

## 适用条件

- 对象的具体类型取决于运行期配置（平台、环境、租户、特性开关）
- 调用方不应知道具体类型，也不应出现按类型的分支
- 同一创建决策需要在多处复用，收敛成一处比复制粘贴好
- 产品类型将来可能扩展，但不希望改动既有调用点

## 反例

- 只有一个实现且永远只有一个——`new()` 就够了
- 需要创建一族互相约束的产品——应改用抽象工厂
- 只是想给一个已有结构体换一组默认值——用 `Default` 或结构体更新语法
- 类型判断逻辑其实属于业务分支而非创建逻辑——用 `match` 直接表达更清晰

## 并发风险

- 返回 `Box<dyn Trait>` 时对象默认不是 `Send`/`Sync`；若要跨线程传递，trait 必须加 `Send`（或 `Send + Sync`）约束
- 工厂方法应保持无状态；把共享缓存放进工厂会让每次创建都变成一次锁竞争
- 若工厂需要读取全局配置，用只读的 `OnceLock`/`RwLock` 快照，不要在工厂里加锁后立刻释放造成 TOCTOU
- 不要用全局单例工厂 + 全局可变状态来"省内存"，那会引入 Singleton 的全部并发风险

## 替代方案

- `pattern.gof.creational.abstract_factory` — 要切换的是一整套相关产品时用抽象工厂，而不是逐个产品的工厂方法。
- `pattern.gof.structural.facade` — 如果真正要解决的是"简化一批子系统的调用"，那应该用外观而不是工厂方法。

## 测试说明

- 对每个具体实现跑同一组契约测试，保证行为对调用方一致
- 断言工厂确实返回了期望的具体类型（例如通过 `Any` 或 trait 方法探针）
- 测试工厂的失败分支（配置缺失、依赖不可用）是否显式报错而非 panic
- 不要让测试依赖工厂内部的构造顺序

## 实现注意

- Rust 里 trait 的静态派发（泛型参数）比 `dyn` 动态派发更快，热路径优先用泛型 + 具体类型
- 工厂方法常与依赖注入组合：工厂负责"造什么"，注入负责"给谁"
- 如果产品的创建参数会持续增长，请与生成器（Builder）配合，而不是继续加工厂方法参数
- 避免"万能工厂"：一个工厂方法只负责一个产品的创建决策

> 许可提醒：本卡正文为本仓库原创（MIT）；上列第三方链接仅作背景参考，其版权与许可需独立核查，不随本仓库的 MIT 授权一并授予。
