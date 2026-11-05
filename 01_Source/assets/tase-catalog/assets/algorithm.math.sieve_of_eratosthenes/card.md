# 埃氏筛（Sieve of Eratosthenes）

- 资产 ID：`algorithm.math.sieve_of_eratosthenes`
- 类型：`algorithm`（算法）
- 分类：`math`
- 版本：`1.0.0`
- 状态：`DOCUMENTED`（知识卡已成文，尚未经人工审阅；本资产没有任何已认证的语言实现）
- 别名：`sieve_of_eratosthenes`、`埃氏筛`、`筛法`、`prime sieve`

## 一句话定位

从 2 开始，把每个素数的倍数标记为合数，直到筛完 √n，剩下未标记的就是素数。

要判断一万个数各自是否为素数，逐个试除要 O(n√n)，筛法一次搞定全部。

## 典型源文件形状

```text
sieve(n):
    is_prime = boolean array of size n+1, filled with true
    is_prime[0] = false; is_prime[1] = false
    p = 2
    while p * p <= n:
        if is_prime[p]:
            for multiple in p*p ..= n step p:
                is_prime[multiple] = false
        p += 1
    return all i where is_prime[i]
```

## 适用条件

- 需要一次性得到 n 以内的全部素数
- n 在百万到亿级（内存放得下标记数组）
- 需要反复查询"某数是否为素数"
- 需要素数密度统计或素数间隔分析

## 反例

- 只判断一个大数是否为素数——用确定性 Miller-Rabin（不需要整个筛表）
- n 极大——分段筛（只筛一段）或线性筛（O(n) 时间）更合适
- 需要因子分解而非素性判断——用试除或 Pollard Rho（注意：这是密码学相关，应使用成熟库）
- n 很小——试除即可

## 并发风险

- 筛法是纯计算、无共享状态，天然线程安全
- 并行化：按分段并行筛（每段先处理小素数再处理段内标记），最后合并
- 奇偶优化（只筛奇数）可把内存减半并提升缓存命中
- 分段筛时每个分段要独立标记小素数的倍数，注意边界处理

## 替代方案

- `algorithm.math.modular_exponentiation` — 需要在大模数下做快速幂时用模幂算法。
- `algorithm.datastructure.segment_tree` — 筛法本身是位数组实现，与线段树无关。

## 测试说明

- 与暴力试除（小 n）交叉比较
- n = 0、n = 1（无素数）、n = 2
- 边界：最大素数附近的数
- 断言筛出的素数全部满足"不被更小的素数整除"
- 性质测试：断言任意素数 p <= n 都在结果中
- 大 n 的性能回归（内存与耗时上限）

## 实现注意

- 优先标准库/成熟库：Rust 标准库没有筛法；成熟 crate 有 `primal`、`sieve` 等，引入前需核查许可
- 复杂度：时间 O(n log log n)，空间 O(n)（位数组可压到 n/8 字节）
- 稳定性：不适用
- 内存：n = 10^8 时布尔数组约 100 MB，用位数组约 12.5 MB
- 溢出：`p * p` 在 p 较大时会溢出——用 `p <= n / p` 形式判断
- 极端输入：n = 0、n = 1、n = 2、n 达到 10^9（内存不可行）
- 并发安全：纯计算；分段并行可行
- 限制：需要把标记数组放入内存；不是确定性素性判定的通用方案

> 许可提醒：本卡正文为本仓库原创（MIT）；上列第三方链接仅作背景参考，其版权与许可需独立核查，不随本仓库的 MIT 授权一并授予。
