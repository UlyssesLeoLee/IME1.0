# KMP 字符串匹配（Knuth–Morris–Pratt Algorithm）

- 资产 ID：`algorithm.string.kmp`
- 类型：`algorithm`（算法）
- 分类：`string`
- 版本：`1.0.0`
- 状态：`DOCUMENTED`（知识卡已成文，尚未经人工审阅；本资产没有任何已认证的语言实现）
- 别名：`KMP`、`Knuth-Morris-Pratt`、`前缀函数匹配`

## 一句话定位

记住模式串"已经匹配了多少前缀"，主串指针永不回退，于是匹配代价与主串长度成正比。

在长文本里反复查找模式串，朴素做法每次失败都要把文本指针退回，复杂度是 O(n·m)。

## 典型源文件形状

```text
build_prefix(pattern):            // 前缀函数 pi[i] = pattern[0..=i] 的最长真前后缀长度
    pi = array of size m
    for i in 1..m:
        j = pi[i-1]
        while j > 0 and pattern[i] != pattern[j]: j = pi[j-1]
        if pattern[i] == pattern[j]: j += 1
        pi[i] = j
    return pi

kmp_search(text, pattern):
    pi = build_prefix(pattern)
    j = 0
    for i in 0..len(text):
        while j > 0 and text[i] != pattern[j]: j = pi[j-1]
        if text[i] == pattern[j]: j += 1
        if j == len(pattern):
            report match at i - len(pattern) + 1
            j = pi[j-1]                 // 继续找重叠匹配
```

## 适用条件

- 在长文本中查找单个模式串的出现位置
- 要求线性时间且模式串会被重复使用
- 需要找到所有匹配位置（含重叠）
- 文本很长、模式串相对固定（可缓存前缀表）

## 反例

- 只匹配一次且模式串极短——朴素匹配的常数更小
- 需要同时匹配大量模式串——用 Aho-Corasick 一次扫描匹配多个
- 模式串每次都变（无法缓存前缀表）——预处理成本会摊不掉
- 需要语义相似匹配（非精确）——KMP 是精确匹配

## 并发风险

- 前缀表是只读数据，计算一次后可被多线程共享（用 `Arc<[usize]>`）
- 匹配过程只读文本与模式串，因此多个线程可并行匹配不同文本
- 若前缀表被缓存在全局可变结构中，更新需要同步
- Unicode 场景下按字节匹配可能命中非法字符边界，应按字符（char）或显式指定编码处理

## 替代方案

- `algorithm.string.z_algorithm` — Z 算法同样线性，且更适合计算所有后缀的匹配长度。
- `algorithm.string.rabin_karp` — Rabin-Karp 用滚动哈希，适合"查找多个可能模式"或需要跳过位置的场合。
- `algorithm.string.aho_corasick` — 多模式匹配用 Aho-Corasick。

## 测试说明

- 与朴素匹配交叉比较，断言所有匹配位置一致（含重叠）
- 模式串为空、文本为空
- 模式串长度 1、模式串长度等于文本长度
- 完全匹配、头部匹配、尾部匹配、无匹配、全重复字符
- 非 ASCII 文本（按字符匹配时）

## 实现注意

- 优先标准库/成熟库：Rust 标准库提供 `str::find`/`str::match_indices`（内部即为高效实现），生产代码不要手写 KMP
- 复杂度：时间 O(n + m)，额外空间 O(m)（前缀表）
- 稳定性：不适用
- 内存：前缀表 O(m)；模式串很大时表也要相应增大
- 溢出：前缀表用 `usize`；长度很大时 O(m) 的内存要先算清
- 极端输入：空模式、空文本、全相同字符、模式串长度远超文本
- 并发安全：只读数据可安全共享；前缀表应一次构建、多次复用
- 限制：单模式、精确匹配；多模式场景不如 Aho-Corasick

> 许可提醒：本卡正文为本仓库原创（MIT）；上列第三方链接仅作背景参考，其版权与许可需独立核查，不随本仓库的 MIT 授权一并授予。
