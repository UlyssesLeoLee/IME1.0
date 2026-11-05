# 生成器（Builder）

- 资产 ID：`pattern.gof.creational.builder`
- 类型：`pattern`（设计模式）
- 分类：`creational`
- 版本：`1.0.0`
- 状态：`DOCUMENTED`（知识卡已成文，尚未经人工审阅；本资产没有任何已认证的语言实现）
- 别名：`Builder`、`生成器模式`、`builder pattern`

## 一句话定位

用一条可读的链式调用把"合法对象的构造"表达出来，把可选参数的组合爆炸收进类型里。

一个对象有 8 个参数、其中 4 个可选、且有几个组合是非法的时，位置参数会变成"记住顺序的谎言"，布尔开关参数更是不可读。

## 典型源文件形状

```rust
#[derive(Debug)]
pub struct HttpRequest {
    method: String,
    url: String,
    headers: Vec<(String, String)>,
    timeout_ms: Option<u64>,
}

pub struct HttpRequestBuilder { req: HttpRequest }

impl HttpRequestBuilder {
    pub fn new(method: &str, url: &str) -> Self {
        Self { req: HttpRequest {
            method: method.into(), url: url.into(),
            headers: Vec::new(), timeout_ms: None,
        } }
    }
    pub fn header(mut self, k: &str, v: &str) -> Self {
        self.req.headers.push((k.into(), v.into())); self
    }
    pub fn timeout_ms(mut self, ms: u64) -> Self { self.req.timeout_ms = Some(ms); self }
    pub fn build(self) -> Result<HttpRequest, BuildError> {
        if self.req.url.is_empty() { return Err(BuildError::MissingUrl); }
        Ok(self.req)
    }
}
```

## 适用条件

- 参数数量多，且大部分调用点只关心其中一两个
- 参数之间存在互斥或依赖规则，需要在构造末尾统一校验
- 同一对象有多种"配置档"（测试用精简构造、生产用完整构造）
- 希望未来新增可选参数时不破坏既有调用点

## 反例

- 参数少且全部必填——结构体字面量更短，builder 增加了一层噪音
- 对象本身是不可变配置且字段少——直接 `Default` + 结构体更新语法（Rust `..Default::default()`）通常更合适
- 构建过程包含大量外部 IO——那不是构造，是命令（Command），应另设对象承载
- 为了"看起来专业"而给每个类型都加 builder——属于以设计模式名义引入不必要的抽象

## 并发风险

- builder 应被消费（`self` by value）而不是共享，不要给它加 `Arc<Mutex<>>` 让多个线程轮流改
- `build(self)` 消费 builder，保证同一份 builder 不会在两个线程里各自产出一个对象
- 若 builder 内含 `Arc<dyn Fn>` 之类的回调，回调本身可能不是 `Send`，会导致下游对象不可跨线程
- builder 里不要做 IO 或启动后台线程；它只应装配数据

## 替代方案

- `pattern.gof.creational.abstract_factory` — 当需求是"换一整套实现"而非"分步装配同一类对象"时改用抽象工厂。
- `pattern.gof.creational.factory_method` — 当需要的是"按条件选一个具体类型"而不是"可选参数多"时改用工厂方法。

## 测试说明

- 对 `build()` 的每条校验规则各写一条失败用例，断言返回的错误类型
- 对默认值、边界值（0、极大值）各写一条成功用例
- 断言"未设置的字段等于默认值"，防止默认值在重构中被改动
- 链式调用要测"不调用 build 直接丢弃"不会产生副作用

## 实现注意

- Rust 中可选字段优先用 `Option<T>` + 结构体更新语法，builder 只在参数多于三个或存在跨字段校验时才划算
- builder 的 `build()` 应返回 `Result`，把校验失败显式暴露，而不是 panic
- 不要让 builder 承担业务逻辑，它只做装配与校验
- 类型化 builder（每个可选参数用不同的零大小标记类型）可以做到"漏传必填参数编译不过"，代价是可读性下降

> 许可提醒：本卡正文为本仓库原创（MIT）；上列第三方链接仅作背景参考，其版权与许可需独立核查，不随本仓库的 MIT 授权一并授予。
