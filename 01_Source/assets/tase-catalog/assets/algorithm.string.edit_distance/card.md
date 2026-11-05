# 编辑距离（Edit Distance (Levenshtein)）

- 资产 ID：`algorithm.string.edit_distance`
- 类型：`algorithm`（算法）
- 分类：`string`
- 版本：`1.0.0`
- 状态：`DOCUMENTED`（知识卡已成文，尚未经人工审阅；本资产没有任何已认证的语言实现）
- 别名：`edit_distance`、`编辑距离`、`Levenshtein`、`levenshtein distance`

## 一句话定位

用动态规划算出把字符串 a 变成 b 的最少编辑次数，矩阵的填法只有三种转移。

拼写纠错、模糊搜索、去重与相似度判断，都需要一个"差多少"的度量。

## 典型源文件形状

```text
edit_distance(a, b):
    m = len(a); n = len(b)
    prev = array of size n+1 filled with 0..n          // 第 0 行
    for i in 1..=m:
        cur = array of size n+1
        cur[0] = i
        for j in 1..=n:
            cost = 0 if a[i-1] == b[j-1] else 1
            cur[j] = min(prev[j] + 1,      // 删除 a[i-1]
                        cur[j-1] + 1,     // 插入 b[j-1]
                        prev[j-1] + cost) // 替换或匹配
        prev = cur
    return prev[n]
```

## 适用条件

- 拼写纠错与模糊搜索
- 字符串相似度判定与去重
- DNA 序列比对（作为基础版本）
- 记录链接与实体对齐

## 反例

- 只需判断相等——直接比较
- 字符串极长——O(n·m) 代价过高，需位并行或启发式方法
- 需要的是语义相似而非字符相似——应改用嵌入或分词
- 允许权重不同的编辑操作——需改用带权版本（DP 转移仍相同，代价参数化）

## 并发风险

- DP 填充是纯计算，无共享状态，可安全并行（按行分块或按对角线并行）
- 并行化时每行依赖上一行，块间需要边界传递；实践中收益有限
- 若把距离缓存到全局结构（如"已比较过的对"），更新需要同步
- Unicode 处理要在分词后进行，不能直接按字节比较，否则"一个字符"可能被算成多次编辑

## 替代方案

- `algorithm.string.kmp` — 需要精确子串匹配时用 KMP；编辑距离是"整体相似度"度量。
- `algorithm.dp.longest_common_subsequence` — LCS 与编辑距离都是 DP，但度量目标不同（保留数 vs 修改数）。

## 测试说明

- 与暴力递归实现（小规模）交叉比较
- 空串与空串、空串与非空串
- 相同字符串（距离 0）、相差一个字符
- Unicode：含组合字符与代理对的字符串（按 char 与按 grapheme 的结果不同，需明确口径）
- 对称性：断言 d(a,b) == d(b,a)
- 三角不等式的抽样检查

## 实现注意

- 优先标准库/成熟库：Rust 有 `str::edit_distance`（`std::str` 的 unstable/实验性 API，视工具链而定）；成熟 crate 有 `strsim`/`levenshtein`，引入前核查许可
- 复杂度：时间 O(n * m)，额外空间 O(min(n, m))（滚动数组）
- 稳定性：不适用
- 内存：两行滚动数组；完整矩阵 O(n·m) 只在需要回溯路径时使用
- 溢出：距离值用 `usize`；`prev[j] + 1` 在极端长度下需检查溢出
- 极端输入：空串、超长串、全部相同字符、Unicode 组合字符
- 并发安全：纯计算，天然可重入；并行化收益有限
- 限制：O(n·m) 时间；标准度量不处理转置（需要 Damerau-Levenshtein）

> 许可提醒：本卡正文为本仓库原创（MIT）；上列第三方链接仅作背景参考，其版权与许可需独立核查，不随本仓库的 MIT 授权一并授予。
