# 二分查找（Binary Search）

- 资产 ID：`algorithm.searching.binary_search`
- 类型：`algorithm`（算法）
- 分类：`searching`
- 版本：`1.0.0`
- 状态：`DOCUMENTED`（知识卡已成文，尚未经人工审阅；本资产没有任何已认证的语言实现）
- 别名：`binary_search`、`二分查找`、`二分搜索`、`折半查找`

## 一句话定位

在有序序列上不断取中点比较，每步把候选范围减半，O(log n) 定位目标。

在百万条有序记录里找一个键，线性扫描要百万次比较，二分只要约 20 次。

## 典型源文件形状

```rust
pub fn binary_search<T: Ord>(a: &[T], target: &T) -> Result<usize, usize> {
    let mut lo = 0usize;
    let mut hi = a.len();          // 半开区间 [lo, hi)
    while lo < hi {
        let mid = lo + (hi - lo) / 2;   // 避免 lo+hi 溢出
        match a[mid].cmp(target) {
            Ordering::Equal => return Ok(mid),
            Ordering::Less => lo = mid + 1,
            Ordering::Greater => hi = mid,
        }
    }
    Err(lo)                        // 插入位置
}
```

## 适用条件

- 数据已按同一比较规则有序
- 查找是静态的或数据不经常变动（变动则要重新排序或换结构）
- 需要精确的位置而不只是"存在与否"
- 内存装不下全部数据（可用流式/分块变体）

## 反例

- 数据无序——先排序通常不划算
- 需要 O(1) 查找且内存够——哈希表更合适
- 数据频繁变动——每次变动都要维护有序结构
- 只需要小范围近似查找——布隆过滤器/位图更省

## 并发风险

- 二分查找是只读操作，共享不可变切片是并发安全的
- 若底层数据在查找过程中被其它线程修改，二分的结果没有意义（读到一半的顺序）
- 共享数据要通过不可变快照（Arc、copy-on-write）提供，不要用 `RwLock` 逐次加锁读
- 对比特（bit）实现里原地二分（原地反转、旋转数组）不是并发安全的

## 替代方案

- `algorithm.searching.lower_bound` — 需要"第一个不小于 x 的位置"（插入点、重复元素起点）时用 lower_bound。
- `algorithm.searching.upper_bound` — 需要"第一个大于 x 的位置"（重复元素终点）时用 upper_bound。
- `algorithm.datastructure.hash_table` — 不要求有序且要 O(1) 查找时用哈希表。

## 测试说明

- 与线性查找交叉比较，断言位置一致
- 目标在首元素、末元素、不存在、大于所有元素、小于所有元素
- 空数组、单元素
- 大量重复元素时断言返回的索引符合约定（任意一个或最左）
- 断言不会 panic：所有下标访问都在界内（可用 fuzz 随机对比）

## 实现注意

- 优先标准库：Rust 的 `slice::binary_search` / `binary_search_by` 已实现且经过验证，不要手写
- 复杂度：时间 O(log n)，额外空间 O(1)
- 稳定性：不适用
- 内存：O(1) 额外空间，不需要复制数据
- 溢出：用 `lo + (hi - lo) / 2` 计算中点，不要用 `(lo + hi) / 2`（`usize` 下会溢出）
- 极端输入：空数组必须安全返回错误而不是 panic；全重复值要明确返回哪一个
- 并发安全：只读操作本身安全，但前提是数据在查找期间不变
- 限制：要求有序；对无序数据无效；无法直接支持"找所有匹配项"（需配合 lower/upper_bound）

> 许可提醒：本卡正文为本仓库原创（MIT）；上列第三方链接仅作背景参考，其版权与许可需独立核查，不随本仓库的 MIT 授权一并授予。
