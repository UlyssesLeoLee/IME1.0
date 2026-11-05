# 装饰（Decorator）

- 资产 ID：`pattern.gof.structural.decorator`
- 类型：`pattern`（设计模式）
- 分类：`structural`
- 版本：`1.0.0`
- 状态：`DOCUMENTED`（知识卡已成文，尚未经人工审阅；本资产没有任何已认证的语言实现）
- 别名：`Decorator`、`装饰器模式`、`decorator pattern`

## 一句话定位

把"附加能力"做成可叠加的包装层，用组合代替继承，在运行期决定叠哪些层、什么顺序。

要给一个已有类型加"日志 + 缓存 + 重试"三种能力：继承会得到 2³ 个子类且相互冲突，组合可以按需叠加。

## 典型源文件形状

```rust
pub trait Storage {
    fn put(&mut self, key: &str, val: &str);
    fn get(&self, key: &str) -> Option<String>;
}

pub struct CachedStorage<S: Storage> { inner: S, cache: std::collections::HashMap<String, String> }

impl<S: Storage> Storage for CachedStorage<S> {
    fn put(&mut self, key: &str, val: &str) {
        self.cache.insert(key.to_owned(), val.to_owned());
        self.inner.put(key, val);
    }
    fn get(&self, key: &str) -> Option<String> {
        self.cache.get(key).cloned().or_else(|| self.inner.get(key))
    }
}
```

## 适用条件

- 需要给对象动态叠加能力，且组合方式在运行期才确定
- 增强之间可以正交叠加（日志 × 缓存 × 重试）
- 不希望修改被装饰对象的源码
- 同一个对象需要不同的增强组合（例如测试时用假实现替换某一层）

## 反例

- 增强只有一种且永远存在——直接改进原类
- 增强逻辑会改变对象的核心语义（不只是"包一层"）——那通常应改原类或换用策略
- 装饰层之间有严格的顺序依赖（缓存包在重试外层与内层行为不同）——顺序敏感时必须显式约束，否则极易出错
- 装饰层数不可控地增长——每层都有一层间接与一次转发开销

## 并发风险

- 装饰链上的每一层都要自己声明线程安全；外层安全不代表内层安全
- 缓存型装饰器内部的可变映射需要 `Mutex`/`RwLock`，否则共享 `Arc<CachedStorage>` 会数据竞争
- 同一装饰链被多线程共享时，缓存的并发命中率与一致性要单独评估；写穿缓存可能造成外层与内层状态不一致
- 重试型装饰器与幂等性：只有幂等操作才能安全重试，否则会重复写入
- 锁顺序：多层装饰若每层各持一把锁，调用方向固定（外→内）可避免死锁，不要在装饰层之间反向回调

## 替代方案

- `pattern.gof.structural.proxy` — 装饰强调"增强并转发"，代理强调"隐藏真实对象"（惰性、权限、远程）；两者结构相似，意图不同。
- `pattern.gof.structural.composite` — 当被装饰对象本身是一组对象（树）时用组合，而不是装饰。

## 测试说明

- 为每个装饰器写单层测试：断言增强确实发生，且未增强部分行为不变
- 组合测试：至少测"日志 + 缓存"两层的顺序敏感行为，把期望顺序写进用例
- 断言装饰链的调用顺序（用 mock 记录调用序列）
- 并发测试：对共享的缓存型装饰器做多线程读写，TSAN 下无告警

## 实现注意

- Rust 用泛型参数（`CachedStorage<S>`）保留静态派发与零成本抽象；需要运行期组装时再退回 `Box<dyn Storage>`
- 装饰层数越多，栈越深、排障越难；把常用组合固化成命名构造函数
- 不要让装饰器改变被装饰对象的可观察语义（相等性、错误类型）而不显式声明
- 装饰是"加行为"，代理是"换实现"；选错会在后续维护中付出代价

> 许可提醒：本卡正文为本仓库原创（MIT）；上列第三方链接仅作背景参考，其版权与许可需独立核查，不随本仓库的 MIT 授权一并授予。
