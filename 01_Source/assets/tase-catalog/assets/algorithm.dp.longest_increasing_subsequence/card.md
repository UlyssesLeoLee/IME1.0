# 最长上升子序列（Longest Increasing Subsequence）

- 资产 ID：`algorithm.dp.longest_increasing_subsequence`
- 类型：`algorithm`（算法）
- 分类：`dp`
- 版本：`1.0.0`
- 状态：`DOCUMENTED`（知识卡已成文，尚未经人工审阅；本资产没有任何已认证的语言实现）
- 别名：`longest_increasing_subsequence`、`LIS`、`最长上升子序列`、`最长递增子序列`

## 一句话定位

维护一个"tails 数组"：tails[k] 记录长度为 k+1 的上升子序列能取得的最小结尾值，配合二分把 O(n²) 降到 O(n log n)。

要找"最长的不下降子序列"，动态规划是 O(n²)，数据上万就太慢。

## 典型源文件形状

```text
lis_length(seq):
    tails = empty vector                 // tails[k] = 长度 k+1 的上升子序列的最小结尾
    for x in seq:
        pos = lower_bound(tails, x)   // 第一个 >= x 的位置
        if pos == len(tails): tails.push(x)
        else: tails[pos] = x
    return len(tails)
```

## 适用条件

- 需要在序列中找最长的递增子序列
- 数据规模大（O(n log n)）
- 子序列不要求连续
- 配套场景：最少不下降分割、俄罗斯套娃信封等

## 反例

- 要求连续子数组——用线性扫描的"最长递增子数组"算法
- 数据量小——O(n²) 的 DP 更直观
- 需要枚举所有最长子序列而不只是长度——O(n log n) 的 tails 方案不直接支持
- 元素可重复且要求非递减——用 `upper_bound` 而不是 `lower_bound`

## 并发风险

- DP 计算是纯函数，可安全并行（按区间分治需要合并技巧）
- tails 数组在并行分块下不能简单拼接，需要用"分治合并"（类似 CDQ）处理
- 只读输入在多线程下共享安全
- 二分查找本身无副作用

## 替代方案

- `algorithm.dp.longest_common_subsequence` — LIS 与 LCS 都是序列 DP，但一个比较元素、一个比较两个序列。
- `algorithm.datastructure.monotonic_stack` — 单调栈处理的是"下一个更大元素"类问题，与 LIS 的 tails 思路不同但同样基于单调性。

## 测试说明

- 与 O(n²) DP 交叉比较长度
- 空序列、单元素、全递增、全递减、全相同
- 含重复值（严格递增 vs 非递减两种口径要分别测）
- 恢复具体子序列（若实现提供还原）：断言它确实是原序列的子序列且严格递增
- 随机序列的长度分布性质

## 实现注意

- 优先标准库/成熟库：`itertools` 有 `sorted_by_key` 但没有直接 LIS；成熟 crate 有 `lis` 等，引入前需核查许可
- 复杂度：时间 O(n log n)，额外空间 O(n)
- 稳定性：不适用
- 内存：tails 数组 O(n)；需要还原路径时还要 O(n) 的前驱数组
- 溢出：元素比较用 `Ord`；若用无符号类型注意语义差异
- 极端输入：空序列、百万元素、严格递增、严格递减、全部相同
- 并发安全：纯计算；并行需要分治合并而非简单分块
- 限制：只给长度最省；还原具体序列需要额外记录；严格性口径必须明确

> 许可提醒：本卡正文为本仓库原创（MIT）；上列第三方链接仅作背景参考，其版权与许可需独立核查，不随本仓库的 MIT 授权一并授予。
