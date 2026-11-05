# Aho-Corasick 多模式匹配（Aho–Corasick Algorithm）

- 资产 ID：`algorithm.string.aho_corasick`
- 类型：`algorithm`（算法）
- 分类：`string`
- 版本：`1.0.0`
- 状态：`DOCUMENTED`（知识卡已成文，尚未经人工审阅；本资产没有任何已认证的语言实现）
- 别名：`aho_corasick`、`AhoCorasick`、`AC自动机`、`多模式匹配`

## 一句话定位

把所有模式串织进一棵带失败指针的字典树，文本只扫一遍就能同时知道"哪些模式在这里出现"。

敏感词库有十万条、病毒特征码有几十万条，逐条匹配文本慢到不可用；一次扫描全部判定才可行。

## 典型源文件形状

```text
build_automaton(patterns):
    trie = insert all patterns into a trie
    build failure links by BFS:
        for node in BFS order:
            for (ch, child) in node.children:
                f = failure[node]
                while f != root and ch not in f.children: f = failure[f]
                failure[child] = f.children.get(ch) or root
                output[child] = output[child] ∪ output[failure[child]]

search(text):
    state = root
    for ch in text:
        while state != root and ch not in state.children: state = failure[state]
        state = state.children.get(ch) or root
        report all patterns in output[state]
```

## 适用条件

- 需要在文本中同时查找大量模式串
- 词法分析的关键字与标识符识别
- 敏感词与安全特征扫描
- 需要报告所有匹配位置（含重叠）

## 反例

- 只有一个模式——用 KMP
- 模式数量少（个位数）——逐个 KMP 的总成本可能更低且内存更省
- 需要模糊匹配或近似匹配——AC 是精确匹配
- 模式数量极大且字符集很大——自动机内存会爆炸

## 并发风险

- 自动机构建后是只读的，可被多线程共享（`Arc`）并并行扫描不同文本
- 扫描状态是每线程局部的（一个整数状态），天然无共享
- 构建阶段通常是单线程的；可并行构建各模式子树但失败链接的 BFS 有依赖
- 输出集合若很大（大量模式同时命中），报告阶段可能成为瓶颈（需要计数上限保护）

## 替代方案

- `algorithm.string.trie` — AC 自动机就是带失败指针的字典树；只需要前缀查询时用普通字典树即可。
- `algorithm.string.kmp` — 单模式匹配时 KMP 更简单。

## 测试说明

- 断言所有模式的所有出现位置都被报告（与暴力匹配交叉比较）
- 重叠匹配（如模式 "aa" 在 "aaa" 中出现两次）
- 模式互为前缀、模式互为后缀
- 空模式串（通常应拒绝或单独处理）
- 文本为空、模式集合为空
- 非 ASCII 文本

## 实现注意

- 优先标准库/成熟库：成熟 crate 有 `aho-corasick`（双许可，需核查选哪一种），引入前确认许可与维护状况
- 复杂度：构建 O(总模式长度 × 字符集)，扫描 O(文本长度 + 匹配数)
- 稳定性：不适用
- 内存：每个节点存转移与输出，O(总模式长度 × 字符集)
- 溢出：节点数与失败指针用 `usize`；极大模式集要评估内存
- 极端输入：十万级模式、模式互为前缀、文本极长、输出量爆炸（需限制单次报告数）
- 并发安全：只读自动机并发安全；每个扫描线程维护自己的状态
- 限制：只做精确匹配；构建时内存占用高；匹配数可能超过文本长度（退化情形要防护）

> 许可提醒：本卡正文为本仓库原创（MIT）；上列第三方链接仅作背景参考，其版权与许可需独立核查，不随本仓库的 MIT 授权一并授予。
