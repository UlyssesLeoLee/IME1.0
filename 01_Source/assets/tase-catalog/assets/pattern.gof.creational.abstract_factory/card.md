# 抽象工厂（Abstract Factory）

- 资产 ID：`pattern.gof.creational.abstract_factory`
- 类型：`pattern`（设计模式）
- 分类：`creational`
- 版本：`1.0.0`
- 状态：`DOCUMENTED`（知识卡已成文，尚未经人工审阅；本资产没有任何已认证的语言实现）
- 别名：`AbstractFactory`、`抽象工厂模式`、`abstract factory`

## 一句话定位

把"造什么"和"怎么造"分开：调用方只面对抽象产品，工厂负责把一整套产品整体换成另一套实现。

同一份业务逻辑要同时跑在两套平台/外观/驱动上时，散落各处的 `match platform` 会让每加一个平台就改一遍全代码，而且容易漏改某条分支。

## 典型源文件形状

```rust
pub trait Button { fn render(&self) -> String; }
pub trait Checkbox { fn render(&self) -> String; }

pub trait UiFactory {
    fn create_button(&self) -> Box<dyn Button>;
    fn create_checkbox(&self) -> Box<dyn Checkbox>;
}

pub struct DarkUi;
impl UiFactory for DarkUi {
    fn create_button(&self) -> Box<dyn Button> { Box::new(DarkButton) }
    fn create_checkbox(&self) -> Box<dyn Checkbox> { Box::new(DarkCheckbox) }
}

pub fn render(factory: &dyn UiFactory) -> String {
    let b = factory.create_button();
    let c = factory.create_checkbox();
    format!("{} {}", b.render(), c.render())
}
```

## 适用条件

- 产品存在两个以上会整体切换的变体（平台、租户、品牌、存储后端）
- 调用方代码不应包含任何按变体分支的 `match`
- 产品之间有强约束关系，必须成套创建，不能被调用方拆开混搭
- 希望新增变体时只新增一个工厂实现，不改动既有调用点

## 反例

- 只有一个产品、且永远只有一种实现——直接用构造函数
- 变体是运行期才确定、且彼此可以自由组合——枚举 + `match` 更直接
- 产品之间没有成套关系，调用方需要自行挑选组合——此时用策略或注册表更合适
- 抽象层无法在不修改公共接口的前提下容纳新变体——说明抽象抽错了维度

## 并发风险

- 工厂对象若被多线程共享，产品类型必须满足 `Send`（跨线程传递）或 `Sync`（共享引用），`Box<dyn Button>` 默认两者都不满足
- 不要用 `static mut` 或裸 `lazy_static` 缓存产品；需要全局单件时用 `OnceLock`，且缓存的对象自身要能承受并发访问
- 工厂方法只应做对象构造，不要在工厂里做 IO 或加锁；把耗时初始化放进产品自身，由调用方决定何时构造
- 工厂实现若持有可变的缓存状态，需用 `Mutex`/`RwLock` 或改为无状态实现，避免出现"工厂成为隐藏的全局锁"

## 替代方案

- `pattern.gof.creational.factory_method` — 只需要在"创建单点产品"时按条件选择实现时用工厂方法，不必为每个产品都定义一个创建 trait。
- `pattern.gof.creational.builder` — 当复杂点其实是一次"分步装配"而不是"成套切换"时，生成器更贴切。

## 测试说明

- 为每个具体工厂写同一个契约测试：对同一族产品调用同一组方法，断言输出一致
- 断言每个工厂都能造出全套产品（防止新增变体时漏实现某个 create_* 方法）
- 用编译期或契约测试确认所有工厂实现满足同一 trait 约束
- 不要断言工厂内部的调用次数或分配次数，这些属于实现细节

## 实现注意

- 抽象工厂是"产品族"概念，工厂方法针对"产品"概念，两者常被混用；选型时先问：要换的是整套还是单个
- trait object 带来动态派发与堆分配，热路径上应改为泛型静态派发或枚举
- 产品族的稳定性是前提：产品集合经常变时，抽象工厂的接口会被反复修改
- 新增产品线时要同步修改所有工厂，这是抽象工厂的固有成本，不要用 trait 默认方法偷偷绕过

> 许可提醒：本卡正文为本仓库原创（MIT）；上列第三方链接仅作背景参考，其版权与许可需独立核查，不随本仓库的 MIT 授权一并授予。
