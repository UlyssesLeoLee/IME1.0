# Programming IME — 设计文档与源码合集（v0.2.0）

- 交付版本：**Code v0.2.0**（2026-10-30）。上游文档：SRS v1.2 / 基本设计 v1.0 / 详细设计 v1.0 / UT、IT、ST v1.0
- 状态：**VS Code 桥接功能完整并通过自动化验收；Rust 核心通过测试；不是 Windows 系统输入法安装包**。Windows TSF 未编译、未验证。

## 目录

- `00_Documents/`：9 个原始设计文件（6 MD + 3 XLSX），内容未修改
- `01_Source/`：源码、测试、Playwright 用户验收脚本（见 `01_Source/README.md`）
- `02_Verification/`：实际运行结果、UT/IT/ST 追踪报告、UAT 截图
- `MANIFEST_SHA256.txt`：文件完整性校验清单

## v0.2 做了什么

1. 七语言声明式规则包 + 上下文感知的本地候选（JS 与 Rust 结果逐项一致）。
2. VS Code 扩展：状态栏语言/AI、语言锁定、候选来源与校验状态、F1 双路径面板（整文件按块 Diff 勾选 / 光标处插入）、Prepare/Apply 事务（版本+哈希复核、读回校验、单步撤销）、BYOK（三类 API、模型/Effort/预算按场景独立、能力不明不发送）、实时 AI（去抖、取消、过期丢弃）、授权与撤销、敏感信息拦截/脱敏、脱敏诊断、三语界面。
3. Rust 核心（零依赖）：同一套语言逻辑、输入状态机、安装计划、JSON Lines 协议 0.2。
4. Playwright UAT：24 个场景在真实 VS Code 1.137.0 上通过。

## 验证事实（2026-10-30，详见 `02_Verification/STATUS.md`）

- JavaScript **81 / 81** 通过；Rust **101 / 101** 通过；Playwright UAT **24 / 24** 通过。
- 设计书用例（按用例编号映射自动化证据）：UT 76/77、IT 38/48、ST 28/43 有通过的自动化证据，0 失败；其余**未执行**（原因逐条见 `02_Verification/test-execution-report.md`）。
- **未执行**：Windows TSF 编译/注册/三语言激活、签名安装器、命名管道 IPC、Credential Manager、真实 AI 服务商、DPI/多屏/读屏、30 分钟离线长稳。
- 设计书的 168 项是计划用例，不是 168 项已通过测试。通过的测试不代表满足全部 64 条需求。

## 安全提醒

AI 默认关闭；开启后按每次 F1 / 每个工作区授权，会把整文件或光标附近代码发送给用户指定的第三方 API。发送前有敏感信息扫描，但不能发现所有秘密。F1 只修改编辑器缓冲区（不自动保存或运行）。
原型不应在生产工作站上注册未签名 TSF DLL；先在隔离 Windows 11 VM 内验证。
