# 责任链（Chain of Responsibility）

- 资产 ID：`pattern.gof.behavioral.chain_of_responsibility`
- 类型：`pattern`（设计模式）
- 分类：`behavioral`
- 版本：`1.0.0`
- 状态：`DOCUMENTED`（知识卡已成文，尚未经人工审阅；本资产没有任何已认证的语言实现）
- 别名：`ChainOfResponsibility`、`责任链模式`、`chain of responsibility`

## 一句话定位

把一串校验/处理步骤串成链，请求沿链下行，命中者就地处理，未命中者可以继续往下传。

一组输入校验规则（长度、字符集、权限、黑名单）层层嵌套 `if`/`else`，加一条规则就要重排所有分支。

## 典型源文件形状

```rust
pub trait Rule {
    fn check(&self, input: &str, next: &dyn Rule) -> Result<(), String>;
}

pub struct LengthRule { min: usize }
impl Rule for LengthRule {
    fn check(&self, input: &str, next: &dyn Rule) -> Result<(), String> {
        if input.len() < self.min { Err("too short".into()) } else { next.check(input, next) }
    }
}

pub struct Chain { first: Box<dyn Rule> }
impl Chain {
    pub fn run(&self, input: &str) -> Result<(), String> {
        self.first.check(input, self.first.as_ref())
    }
}
```

## 适用条件

- 一组规则会持续增减，且每条规则的判定互相独立
- 处理者之间不需要互相知道，只需要知道"下一个"
- 希望顺序即优先级，且顺序本身是可配置的数据
- 一个请求可能需要穿过多个处理者（例如校验 + 改写 + 记录）

## 反例

- 规则少且固定——分支判断更短、更容易穷尽检查
- 处理者需要互相看见对方的状态——那不是链，是协作对象图
- 链在运行期需要频繁增删节点——静态链结构会变成性能与可读性负担
- 所有处理者都会执行（而非首个命中即停）时，链退化成一次遍历，责任链的"可插拔"意义减弱

## 并发风险

- 链通常构建一次后只读共享：`Box<dyn Rule>` 默认不满足 `Send`/`Sync`，要跨线程必须给 trait 加约束
- 若处理者持有可变状态（如计数器、缓存），链共享时这些状态必须同步，否则出现难以复现的竞争
- 链中的处理者若会调用外部回调，回调的线程安全性会影响整条链
- 链长度会影响延迟上界，超长链应考虑并行化或分层短路

## 替代方案

- `pattern.gof.behavioral.interpreter` — 当"匹配规则"本身需要被解析成表达式时，用解释器代替硬编码的链。
- `pattern.gof.structural.adapter` — 当各规则接口不一致、需要一个统一入口时先用适配器统一，再串成链。

## 测试说明

- 每条规则写独立用例：命中时链在预期位置停止，未命中时继续传递
- 断言链的顺序即优先级：调整顺序后结果应随之改变
- 测试空链与单节点链的边界行为（应明确返回"未处理"而不是 panic）
- 断言链能终止：故意构造一个总是转发的节点，检查有最大跳数或无环保证

## 实现注意

- 责任链的经典形态是"首个匹配即处理"；要"全部处理"时应明确命名，避免与过滤器链混淆
- Rust 的 `&dyn Trait` 递归链写法易出错，可用 `Box<dyn Rule>` + `next` 字段，或改用迭代器链（`Iterator::chain`）
- 链的顺序应来自配置或数据（可测试），而不是散落在代码里的构造顺序
- 链里每条规则都应便宜，短路要早

> 许可提醒：本卡正文为本仓库原创（MIT）；上列第三方链接仅作背景参考，其版权与许可需独立核查，不随本仓库的 MIT 授权一并授予。
